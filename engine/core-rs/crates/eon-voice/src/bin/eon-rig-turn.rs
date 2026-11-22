//! A session on one selected speaker. The recorder supplies only the authorized, live input reference.
//! Never open a microphone, pad an unfinished turn at EOF, or interpret a partial as a command.

use std::error::Error;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::Ordering::Acquire;
use std::time::{Duration, Instant};

use eon_voice::asr::ZipformerAsr;
use eon_voice::barge_in::BargeIn;
use eon_voice::commitment::{Bridge, BridgeCache, CommitmentGate};
use eon_voice::device::play_selected_output;
use eon_voice::geometry::VAD_FRAME_SAMPLES;
use eon_voice::pre_cognition::IntentPreview;
use eon_voice::resample::CaptureFrames;
use eon_voice::ring::{PcmProducer, pcm_ring};
use eon_voice::tts::KokoroTts;
use eon_voice::vad::{SileroVad, VadEndpoint};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn reply(
    input: impl Read,
    rate: u32,
    vad: SileroVad,
    asr: &mut ZipformerAsr,
    ready: impl FnOnce() -> Result<()>,
) -> Result<Option<(Bridge, u32)>> {
    let mut endpoint = VadEndpoint::new(vad);
    let mut stream = asr.start_utterance()?;
    let mut partial = String::new();
    let (mut tx, rx) = pcm_ring(1 << 14);
    let mut frames = CaptureFrames::<VAD_FRAME_SAMPLES>::new(rx, rate)?;
    let mut input = io::BufReader::new(input);
    let mut bytes = [0; 512];
    let mut reference_frames = 0;
    ready()?;
    loop {
        // Arbitrarily fragmented pipe reads may split an i16.
        let mut n = input.read(&mut bytes)?;
        if n == 0 {
            return Ok(None);
        }
        if n % 2 != 0 {
            input.read_exact(&mut bytes[n..=n])?;
            n += 1;
        }
        let samples = bytes[..n].as_chunks::<2>().0;
        // These capture-clock frames were already recorded before enqueue. Do
        // not count EOF padding, buffered lookahead, or the post-reply drain.
        reference_frames += samples.len() as u32;
        let mut pcm = [0.; 256];
        for (dst, pair) in pcm.iter_mut().zip(samples) {
            *dst = f32::from(i16::from_le_bytes(*pair)) / 32768.;
        }
        if tx.push_slice(&pcm[..samples.len()]) != samples.len() {
            return Err("discontinuous input".into());
        }
        while let Some(frame) = frames.try_read()? {
            if let Some(update) = stream.push(&frame)? {
                partial = update.text;
            }
            if endpoint.push(&frame, &partial)?.is_some() {
                let text = stream.finish()?;
                if text.trim().is_empty() {
                    return Ok(None);
                }
                let bridge = match IntentPreview::from_partial(&text) {
                    Some(IntentPreview::Question) => Bridge::Report,
                    Some(IntentPreview::Request) => Bridge::Proposal,
                    _ => Bridge::Notice,
                };
                return Ok(Some((bridge, reference_frames)));
            }
        }
    }
}

// Protocol v3: ready '3', then either 'P' (whole prefix consumed by the selected
// output callback) and terminal 'B' + u32 LE reference frames, or just 'N'. Keep reading the
// bounded take during playback; never wait for the renderer in the audio callback.
fn finish_take(
    pcm: &mut dyn Read,
    output: &mut dyn Write,
    prefix_end: Option<u64>,
    mut rendered: impl FnMut() -> Result<u64>,
) -> Result<()> {
    let mut pending = prefix_end;
    let mut observe = || -> Result<bool> {
        let count = rendered()?;
        if pending.is_some_and(|end| count >= end) {
            status(output, b'P')?;
            pending = None;
        }
        Ok(pending.is_none())
    };
    let mut block = [0; 8192];
    while pcm.read(&mut block)? != 0 {
        observe()?;
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while !observe()? {
        if Instant::now() >= deadline {
            return Err("selected speaker stopped consuming PCM".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

// u32 little-endian frame count, exactly that many mono i16 samples. Zero ends the
// session. Never interpret a short take as an endpoint or read into the next header.
fn serve(
    input: &mut impl Read,
    output: &mut impl Write,
    rate: u32,
    mut run: impl FnMut(&mut dyn Read, &mut dyn Write) -> Result<Option<u32>>,
) -> Result<()> {
    loop {
        let mut header = [0; 4];
        input.read_exact(&mut header)?;
        let frames = u32::from_le_bytes(header);
        if frames == 0 {
            return Ok(());
        }
        if frames > rate * 30 {
            return Err("take exceeds 30 seconds".into());
        }
        let mut pcm = input.take(u64::from(frames) * 2);
        let reference_frames = run(&mut pcm, output)?;
        if pcm.limit() != 0 {
            return Err("truncated reference take".into());
        }
        if let Some(n) = reference_frames {
            if !(1..=frames).contains(&n) {
                return Err("invalid pre-enqueue reference count".into());
            }
            output.write_all(b"B")?;
            output.write_all(&n.to_le_bytes())?;
            output.flush()?;
        } else {
            status(output, b'N')?;
        }
    }
}

fn status(output: &mut dyn Write, byte: u8) -> Result<()> {
    output.write_all(&[byte])?;
    Ok(output.flush()?)
}

// Only cached prefixes, never free text. The ring is empty at each start and
// must fit the whole prefix. No pending render or decoder state crosses takes.
fn start_prefix(
    tx: &mut PcmProducer,
    gate: &CommitmentGate,
    cache: &BridgeCache,
    bridge: Bridge,
) -> Result<usize> {
    let samples = gate.bridge(cache, bridge).ok_or("bridge not permitted")?;
    if samples.is_empty() || tx.push_slice(samples) != samples.len() {
        return Err("cached prefix did not fit the output ring".into());
    }
    Ok(samples.len())
}

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let [flag, rate, speaker, models] = args.as_slice() else {
        return Err(
            "usage: eon-rig-turn --live-reference-session RATE alsa:hw:CARD=NAME,DEV=N MODEL_DIRECTORY"
                .into(),
        );
    };
    let rate = rate.parse()?;
    let hardware = speaker
        .strip_prefix("alsa:hw:CARD=")
        .and_then(|s| s.split_once(",DEV="));
    if flag != "--live-reference-session"
        || !matches!(rate, 16000 | 44100 | 48000 | 96000)
        || !hardware.is_some_and(|(card, dev)| {
            !card.is_empty()
                && card
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
                && (1..=3).contains(&dev.len())
                && dev.bytes().all(|c| c.is_ascii_digit())
        })
    {
        return Err("expected explicit rig input format and named ALSA hardware output".into());
    }
    let id = speaker.parse()?;
    let models = Path::new(models);
    let vad = models.join("vad/silero_vad.onnx");
    let mut asr = ZipformerAsr::load(models.join("asr"))?;
    let mut tts = KokoroTts::load(
        models.join("tts/release/kokoro-v1.0.int8.onnx"),
        models.join("tts/voices/af_heart.bin"),
    )?;
    let (mut tx, rx) = pcm_ring(1 << 19);
    let signal = BargeIn::new();
    let device = play_selected_output(&id, rx, signal.clone())?;
    let cache = BridgeCache::warm_for_output(&mut tts, 1., device.sample_rate)?;
    let healthy = || -> Result<()> {
        if device.counters.driver_errors.load(Acquire) != 0 {
            return Err("selected speaker disconnected".into());
        }
        Ok(())
    };
    let gate = CommitmentGate::new(1, signal);
    let mut submitted = 0;
    serve(
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        rate,
        |pcm, output| {
            let decision = reply(&mut *pcm, rate, SileroVad::load(&vad)?, &mut asr, || {
                healthy()?;
                status(output, b'3')
            })?;
            let prefix_end = if let Some((bridge, _)) = decision {
                healthy()?;
                submitted += start_prefix(&mut tx, &gate, &cache, bridge)? as u64;
                Some(submitted)
            } else {
                None
            };
            finish_take(pcm, output, prefix_end, || {
                healthy()?;
                Ok(device.counters.frames.load(Acquire))
            })?;
            Ok(decision.map(|(_, n)| n))
        },
    )?;
    healthy()?;
    status(&mut io::stdout().lock(), b'D')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_payload_or_header_never_reports_an_outcome() {
        for mut data in [&[2, 0, 0, 0, 1, 0][..], &[0, 1], &u32::MAX.to_le_bytes()] {
            let mut replies = Vec::new();
            let failed = serve(&mut data, &mut replies, 16000, |pcm, output| {
                status(output, b'3')?;
                io::copy(pcm, &mut io::sink())?;
                Ok(None)
            });
            assert!(failed.is_err());
            assert!(replies.is_empty() || replies == b"3");
        }
    }

    #[test]
    fn each_started_take_needs_a_reference_count_within_that_take() {
        for (count, expected) in [
            (None, Some(&b"N"[..])),
            (Some(2), Some(&b"B\x02\x00\x00\x00"[..])),
            (Some(0), None),
            (Some(3), None),
        ] {
            let mut input = [2, 0, 0, 0, 9, 8, 7, 6, 0, 0, 0, 0].as_slice();
            let mut output = Vec::new();
            let result = serve(&mut input, &mut output, 16000, |pcm, _| {
                io::copy(pcm, &mut io::sink())?;
                Ok(count)
            });
            assert_eq!(result.is_ok(), expected.is_some());
            assert_eq!(output, expected.unwrap_or_default());
        }
    }

    #[test]
    fn marker_waits_for_this_prefix_and_is_sent_before_terminal_outcome() -> Result<()> {
        let payload = [0u8; 16384]; // two drain reads and one EOF observation
        let mut replies = Vec::new();
        let mut counts = [17, 18, 19].into_iter();
        let rendered = || Ok(counts.next().unwrap());
        finish_take(&mut &payload[..], &mut replies, Some(18), rendered)?;
        assert_eq!(replies, b"P");
        // Lose the driver on the last (EOF) check: only a complete render observed
        // during input drain can already have emitted P, but neither may emit B.
        for (last_live, expected) in [(17, &b""[..]), (18, b"P")] {
            replies.clear();
            let mut counts = [Ok(17), Ok(last_live), Err("lost output".into())].into_iter();
            let rendered = || counts.next().unwrap();
            assert!(finish_take(&mut &payload[..], &mut replies, Some(18), rendered).is_err());
            assert_eq!(replies, expected);
        }
        Ok(())
    }

    // Explicit native model test. The output ring here is not a physical latency measurement.
    #[test]
    #[ignore = "requires digest-verified Silero, Zipformer, Kokoro release and recorded speech"]
    fn recorded_turn_reaches_renderer_but_silence_and_truncated_input_do_not() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let models = root.join("../../../../perception/models");
        let mut asr = ZipformerAsr::load(models.join("asr"))?;
        let mut tts = KokoroTts::load(
            models.join("tts/release/kokoro-v1.0.int8.onnx"),
            models.join("tts/voices/af_heart.bin"),
        )?;
        let cache = BridgeCache::warm(&mut tts, 1.)?;
        let wav = std::fs::read(root.join("tests/fixtures/en.wav"))?;
        let mut chunks = &wav[12..];
        let speech = loop {
            let size = u32::from_le_bytes(chunks[4..8].try_into()?) as usize;
            if &chunks[..4] == b"data" {
                break &chunks[8..8 + size];
            }
            chunks = &chunks[8 + size + size % 2..];
        };
        let mut input = Vec::new();
        let mut complete = speech.to_vec();
        complete.extend_from_slice(&[0; 64000]); // actual trailing input, not synthesized by the responder
        for pcm in [&complete[..], &speech[..8000], &[0; 64000], &complete] {
            input.extend_from_slice(&((pcm.len() / 2) as u32).to_le_bytes());
            input.extend_from_slice(pcm);
        }
        input.extend_from_slice(&0u32.to_le_bytes());
        let mut replies = Vec::new();
        let (mut tx, mut rx) = pcm_ring(1 << 19);
        let gate = CommitmentGate::new(1, BargeIn::new());
        serve(&mut &input[..], &mut replies, 16000, |pcm, output| {
            let decision = reply(
                &mut *pcm,
                16000,
                SileroVad::load(models.join("vad/silero_vad.onnx"))?,
                &mut asr,
                || status(output, b'3'),
            )?;
            let prefix_end = decision
                .map(|(b, _)| start_prefix(&mut tx, &gate, &cache, b))
                .transpose()?;
            let mut block = vec![0.; rx.available()];
            let rendered = rx.pop_slice(&mut block);
            assert_eq!(block.iter().any(|s| s.abs() > 0.01), prefix_end.is_some());
            finish_take(pcm, output, prefix_end.map(|n| n as u64), || {
                Ok(rendered as u64)
            })?;
            Ok(decision.map(|(_, n)| n))
        })?;
        assert_eq!(replies.len(), 18);
        assert_eq!(&replies[..3], b"3PB");
        assert_eq!(&replies[7..14], b"3N3N3PB");
        let frames = u32::from_le_bytes(replies[3..7].try_into()?);
        assert!((1..complete.len() / 2).contains(&(frames as usize)));
        assert_eq!(replies[3..7], replies[14..18]);
        assert!(
            reply(
                &[1u8][..],
                44100,
                SileroVad::load(models.join("vad/silero_vad.onnx"))?,
                &mut asr,
                || Ok(())
            )
            .is_err()
        );
        Ok(())
    }
}
