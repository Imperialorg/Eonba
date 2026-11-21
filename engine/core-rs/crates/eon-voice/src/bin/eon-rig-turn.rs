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
    mut start: impl FnMut(Bridge) -> Result<()>,
) -> Result<bool> {
    let mut endpoint = VadEndpoint::new(vad);
    let mut stream = Some(asr.start_utterance()?);
    let mut partial = String::new();
    let (mut tx, rx) = pcm_ring(1 << 14);
    let mut frames = CaptureFrames::<VAD_FRAME_SAMPLES>::new(rx, rate)?;
    let mut input = io::BufReader::new(input);
    let mut started = false;
    let mut bytes = [0; 512];
    ready()?;
    loop {
        // Arbitrarily fragmented pipe reads may split an i16.
        let mut n = input.read(&mut bytes)?;
        if n == 0 {
            return Ok(started);
        }
        if n % 2 != 0 {
            input.read_exact(&mut bytes[n..=n])?;
            n += 1;
        }
        if stream.is_some() {
            let samples = bytes[..n].as_chunks::<2>().0;
            let mut pcm = [0.; 256];
            for (dst, pair) in pcm.iter_mut().zip(samples) {
                *dst = f32::from(i16::from_le_bytes(*pair)) / 32768.;
            }
            if tx.push_slice(&pcm[..samples.len()]) != samples.len() {
                return Err("discontinuous input".into());
            }
            while let Some(frame) = frames.try_read()? {
                let decoder = stream.as_mut().unwrap();
                if let Some(update) = decoder.push(&frame)? {
                    partial = update.text;
                }
                if endpoint.push(&frame, &partial)?.is_some() {
                    let text = stream.take().unwrap().finish()?;
                    if !text.trim().is_empty() {
                        start(match IntentPreview::from_partial(&text) {
                            Some(IntentPreview::Question) => Bridge::Report,
                            Some(IntentPreview::Request) => Bridge::Proposal,
                            _ => Bridge::Notice,
                        })?;
                        started = true;
                    }
                    break;
                }
            }
        }
    }
}

// u32 little-endian frame count, exactly that many mono i16 samples. Zero ends the
// session. Never interpret a short take as an endpoint or read into the next header.
fn serve(
    input: &mut impl Read,
    output: &mut impl Write,
    rate: u32,
    mut run: impl FnMut(&mut dyn Read, &mut dyn Write) -> Result<bool>,
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
        let started = run(&mut pcm, output)?;
        if pcm.limit() != 0 {
            return Err("truncated reference take".into());
        }
        status(output, if started { b'B' } else { b'N' })?;
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
            let started = reply(
                pcm,
                rate,
                SileroVad::load(&vad)?,
                &mut asr,
                || {
                    healthy()?;
                    status(output, b'R')
                },
                |bridge| {
                    healthy()?;
                    submitted += start_prefix(&mut tx, &gate, &cache, bridge)? as u64;
                    Ok(())
                },
            )?;
            let deadline = Instant::now() + Duration::from_secs(3);
            while device.counters.frames.load(Acquire) < submitted {
                healthy()?;
                if Instant::now() >= deadline {
                    return Err("selected speaker stopped consuming PCM".into());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            healthy()?;
            Ok(started)
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
                status(output, b'R')?;
                io::copy(pcm, &mut io::sink())?;
                Ok(false)
            });
            assert!(failed.is_err());
            assert!(replies.is_empty() || replies == b"R");
        }
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
            let started = reply(
                pcm,
                16000,
                SileroVad::load(models.join("vad/silero_vad.onnx"))?,
                &mut asr,
                || status(output, b'R'),
                |bridge| {
                    start_prefix(&mut tx, &gate, &cache, bridge)?;
                    Ok(())
                },
            )?;
            let mut block = vec![0.; rx.available()];
            rx.pop_slice(&mut block);
            assert_eq!(block.iter().any(|s| s.abs() > 0.01), started);
            Ok(started)
        })?;
        assert_eq!(replies, b"RBRNRNRB");
        assert!(
            reply(
                &[1u8][..],
                44100,
                SileroVad::load(models.join("vad/silero_vad.onnx"))?,
                &mut asr,
                || Ok(()),
                |_| panic!("must not render a truncated sample")
            )
            .is_err()
        );
        Ok(())
    }
}
