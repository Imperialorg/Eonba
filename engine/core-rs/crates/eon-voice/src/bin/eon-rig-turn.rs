//! One local benchmark turn. The recorder supplies only the authorized, live input reference.
//! Never open a microphone, pad an unfinished turn at EOF, or interpret a partial as a command.

use std::error::Error;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::Ordering::Acquire;

use eon_voice::asr::ZipformerAsr;
use eon_voice::barge_in::BargeIn;
use eon_voice::commitment::{Bridge, BridgeCache, ReplyRenderer};
use eon_voice::device::play_selected_output;
use eon_voice::geometry::VAD_FRAME_SAMPLES;
use eon_voice::pre_cognition::IntentPreview;
use eon_voice::resample::CaptureFrames;
use eon_voice::ring::pcm_ring;
use eon_voice::tts::KokoroTts;
use eon_voice::vad::{SileroVad, VadEndpoint};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn reply(
    input: impl Read,
    rate: u32,
    vad: SileroVad,
    asr: &mut ZipformerAsr,
    ready: impl FnOnce() -> Result<()>,
    mut render: impl FnMut(Option<Bridge>) -> Result<()>,
) -> Result<()> {
    let mut endpoint = VadEndpoint::new(vad);
    let mut stream = Some(asr.start_utterance()?);
    let mut partial = String::new();
    let (mut tx, rx) = pcm_ring(1 << 14);
    let mut frames = CaptureFrames::<VAD_FRAME_SAMPLES>::new(rx, rate)?;
    let mut input = io::BufReader::new(input);
    let mut sample_count = 0u64;
    let mut bytes = [0; 512];
    ready()?;
    loop {
        // Arbitrarily fragmented pipe reads may split an i16.
        let mut n = input.read(&mut bytes)?;
        if n == 0 {
            return Ok(());
        }
        if n % 2 != 0 {
            input.read_exact(&mut bytes[n..=n])?;
            n += 1;
        }
        sample_count += (n / 2) as u64;
        if sample_count > u64::from(rate) * 30 {
            return Err("take exceeds 30 seconds".into());
        }
        let mut bridge = None;
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
                        bridge = Some(match IntentPreview::from_partial(&text) {
                            Some(IntentPreview::Question) => Bridge::Report,
                            Some(IntentPreview::Request) => Bridge::Proposal,
                            _ => Bridge::Notice,
                        });
                    }
                    break;
                }
            }
        }
        render(bridge)?;
    }
}

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let [flag, rate, speaker, models] = args.as_slice() else {
        return Err(
            "usage: eon-rig-turn --live-reference RATE alsa:hw:CARD=NAME,DEV=N MODEL_DIRECTORY"
                .into(),
        );
    };
    let rate = rate.parse()?;
    if flag != "--live-reference"
        || !matches!(rate, 16000 | 44100 | 48000 | 96000)
        || !speaker.starts_with("alsa:hw:CARD=")
    {
        return Err("expected explicit rig input format and named ALSA hardware output".into());
    }
    let id = speaker.parse()?;
    let models = Path::new(models);
    let vad = SileroVad::load(models.join("vad/silero_vad.onnx"))?;
    let mut asr = ZipformerAsr::load(models.join("asr"))?;
    let mut tts = KokoroTts::load(
        models.join("tts/release/kokoro-v1.0.int8.onnx"),
        models.join("tts/voices/af_heart.bin"),
    )?;
    let (tx, rx) = pcm_ring(1 << 15);
    let signal = BargeIn::new();
    let device = play_selected_output(&id, rx, signal.clone())?;
    let cache = BridgeCache::warm_for_output(&mut tts, 1., device.sample_rate)?;
    let mut renderer = ReplyRenderer::new(1, signal, tx, device.sample_rate)?;
    reply(
        io::stdin().lock(),
        rate,
        vad,
        &mut asr,
        || {
            println!("READY");
            Ok(io::stdout().flush()?)
        },
        |bridge| {
            if device.counters.driver_errors.load(Acquire) != 0 {
                return Err("selected speaker disconnected".into());
            }
            if let Some(bridge) = bridge {
                renderer.start_bridge(&cache, bridge)?;
            }
            renderer.pump(&mut tts, 1.)?;
            Ok(())
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for (mut pcm, expected) in [(speech.to_vec(), true), (vec![0; 32000], false)] {
            pcm.extend_from_slice(&[0; 64000]); // actual trailing input, not synthesized by the responder
            let (tx, mut rx) = pcm_ring(1 << 15);
            let mut renderer = ReplyRenderer::new(1, BargeIn::new(), tx, 24000)?;
            let mut count = 0;
            let mut audible = false;
            reply(
                &pcm[..],
                16000,
                SileroVad::load(models.join("vad/silero_vad.onnx"))?,
                &mut asr,
                || Ok(()),
                |bridge| {
                    if let Some(bridge) = bridge {
                        count += 1;
                        renderer.start_bridge(&cache, bridge)?;
                    }
                    renderer.pump(&mut tts, 1.)?;
                    let mut block = [0.; 1024];
                    let n = rx.pop_slice(&mut block);
                    audible |= block[..n].iter().any(|s| s.abs() > 0.01);
                    Ok(())
                },
            )?;
            assert_eq!((count, audible), (usize::from(expected), expected));
        }
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
