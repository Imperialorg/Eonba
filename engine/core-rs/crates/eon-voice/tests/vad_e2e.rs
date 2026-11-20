//! End-to-end VAD test: pinned Silero v5 ONNX model over a real 16 kHz speech recording.
//!
//! The model comes from `scripts/models.lock.json` (`silero-vad-v5`). Put it at the repo path below, or set
//! `EON_SILERO_MODEL`. The WAV fixture is the upstream `test_wavs/en.wav` (sha256 eb1eb008...).

use std::path::PathBuf;

mod common;

use common::read_pcm16_mono_16k;
use eon_voice::endpoint::{Completeness, Endpointer};
use eon_voice::geometry::VAD_FRAME_SAMPLES;
use eon_voice::vad::SileroVad;

fn model_path() -> PathBuf {
    if let Ok(path) = std::env::var("EON_SILERO_MODEL") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../perception/models/vad/silero_vad.onnx")
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn load_model() -> SileroVad {
    let path = model_path();
    assert!(
        path.exists(),
        "missing {}; run: python3 scripts/download_models.py --ids silero-vad-v5",
        path.display()
    );
    SileroVad::load(path).expect("pinned Silero model must load")
}

#[test]
fn callback_sized_fragments_of_recorded_speech_reach_the_endpoint_at_device_rate() {
    use eon_voice::resample::{CaptureFrames, resample_mono};
    use eon_voice::ring::pcm_ring;
    use eon_voice::vad::{VadEndpoint, VadStep};

    let speech = read_pcm16_mono_16k(&fixture("en.wav"));
    let mut device = resample_mono(&speech, 16_000, 44_100).unwrap();
    device.resize(device.len() + 2 * 44_100, 0.0);
    let (mut tx, rx) = pcm_ring(4096);
    let mut input = CaptureFrames::new(rx, 44_100).unwrap();
    let mut endpoint = VadEndpoint::new(load_model());
    let mut endpoints = Vec::new();
    for callback in device.chunks(193) {
        assert_eq!(tx.push_slice(callback), callback.len());
        while let VadStep::Processed(event) = endpoint.poll_capture(&mut input, "").unwrap() {
            endpoints.extend(event);
        }
    }
    assert!(!endpoints.is_empty());
    assert!(endpoints.iter().all(|e| e.silence_ms == 800));
}

#[cfg(feature = "native")]
#[test]
#[ignore = "requires an explicitly configured virtual monitor; never run against an ambient microphone"]
fn virtual_cpal_monitor_runs_the_listening_endpoint() {
    use eon_voice::barge_in::BargeIn;
    use eon_voice::device::{capture_default_frames, play_default_output};
    use eon_voice::resample::resample_mono;
    use eon_voice::ring::pcm_ring;
    use eon_voice::vad::{VadEndpoint, VadStep};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    assert_eq!(
        std::env::var("EON_VOICE_TEST_VIRTUAL_MONITOR").as_deref(),
        Ok("1"),
        "opt in only after configuring local virtual loopback"
    );
    let path = std::env::var_os("EON_ZIPFORMER_TEST_WAV_0")
        .map(PathBuf::from)
        .unwrap_or_else(|| fixture("0.wav"));
    let speech = read_pcm16_mono_16k(&path);
    let mut endpoint = VadEndpoint::new(load_model());
    let (mut tx, rx) = pcm_ring(1 << 21);
    let output = play_default_output(rx, BargeIn::new()).unwrap();
    let source = resample_mono(&speech, 16_000, output.sample_rate).unwrap();
    let (mut reader, input) = capture_default_frames().unwrap();
    assert_eq!(tx.push_slice(&source), source.len());
    let start = Instant::now();
    let mut frames = 0;
    loop {
        assert!(start.elapsed() < Duration::from_secs(12), "no endpoint after {frames} frames");
        match endpoint.poll_capture(&mut reader, "").unwrap() {
            VadStep::Waiting => std::thread::sleep(Duration::from_micros(250)),
            VadStep::Processed(event) => {
                frames += 1;
                if let Some(event) = event {
                    assert_eq!(event.silence_ms, 800);
                    break;
                }
            }
        }
    }
    assert_eq!(input.counters.capture_drops.load(Ordering::Acquire), 0);
    assert_eq!(input.counters.driver_errors.load(Ordering::Acquire), 0);
    assert_eq!(output.counters.driver_errors.load(Ordering::Acquire), 0);
    eprintln!("CPAL virtual monitor {} Hz -> Silero endpoint: {frames} frames", input.sample_rate);
}

#[test]
fn endpoint_runs_on_released_speech_after_post_reply_session_split() {
    use eon_voice::vad::VadEndpoint;
    let path = std::env::var_os("EON_ZIPFORMER_TEST_WAV_0")
        .map(PathBuf::from)
        .unwrap_or_else(|| fixture("0.wav"));
    let samples = read_pcm16_mono_16k(&path);
    let mut endpoint = VadEndpoint::new(load_model());
    let previous_session = endpoint.session().clone();
    endpoint.reply_finished();
    assert_ne!(&previous_session, endpoint.session());
    let silence = [0.0; VAD_FRAME_SAMPLES];
    for _ in 0..10 {
        assert!(endpoint.push(&silence, "").unwrap().is_none());
    }
    let mut endpoints = 0;
    for frame in samples
        .as_chunks::<VAD_FRAME_SAMPLES>()
        .0
        .iter()
        .chain(std::iter::repeat_n(&silence, 60))
    {
        endpoints += usize::from(endpoint.push(frame, "after early nightfall").unwrap().is_some());
    }
    assert!(endpoints > 0, "real released speech plus silence never endpointed");
    let mut vad = load_model();
    let mut durations = Vec::new();
    let mut voiced = 0;
    for frame in samples.as_chunks::<VAD_FRAME_SAMPLES>().0 {
        let start = std::time::Instant::now();
        let p = vad.speech_probability(frame).unwrap();
        durations.push(start.elapsed());
        voiced += usize::from(p > 0.5);
    }
    assert!(voiced > durations.len() / 3, "missing speech in the released clip");
    durations.sort();
    eprintln!(
        "Silero {} frames: p50={:?} p95={:?} max={:?}",
        durations.len(),
        durations[durations.len() / 2],
        durations[durations.len() * 95 / 100],
        durations.last().unwrap()
    );
}

#[test]
fn silence_scores_low() {
    let mut vad = load_model();
    let silence = vec![0.0f32; VAD_FRAME_SAMPLES];
    let p = vad.speech_probability(&silence).unwrap();
    assert!((0.0..=1.0).contains(&p));
    assert!(p < 0.1, "silence scored {p}");
}

#[test]
fn wrong_frame_size_is_refused_before_inference() {
    let mut vad = load_model();
    assert!(vad.speech_probability(&[0.0; 480]).is_err());
}

#[test]
fn invalid_pcm_is_rejected_during_holdoff_and_preserves_inference_state() {
    use eon_voice::vad::VadEndpoint;
    let samples = read_pcm16_mono_16k(&fixture("en.wav"));
    let frames = samples.as_chunks::<VAD_FRAME_SAMPLES>().0;
    let mut vad = load_model();
    let mut control = load_model();
    let mut endpoint = VadEndpoint::new(load_model());
    endpoint.reply_finished();
    for frame in &frames[..10] {
        assert_eq!(
            vad.speech_probability(frame).unwrap(),
            control.speech_probability(frame).unwrap()
        );
    }
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.01, -1.01] {
        let mut frame = frames[10];
        frame[VAD_FRAME_SAMPLES - 1] = invalid;
        assert!(vad.speech_probability(&frame).is_err());
        assert!(endpoint.push(&frame, "").is_err());
    }
    for frame in &frames[10..20] {
        assert_eq!(
            vad.speech_probability(frame).unwrap(),
            control.speech_probability(frame).unwrap()
        );
    }
}

#[test]
fn reset_makes_inference_repeatable() {
    let mut vad = load_model();
    let frame: Vec<f32> = (0..VAD_FRAME_SAMPLES)
        .map(|i| ((i as f32) * 0.05).sin() * 0.3)
        .collect();
    let first = vad.speech_probability(&frame).unwrap();
    vad.reset();
    let again = vad.speech_probability(&frame).unwrap();
    assert_eq!(first, again);
}

#[test]
fn real_speech_is_detected_and_its_pauses_are_scored_as_silence() {
    let mut vad = load_model();
    let samples = read_pcm16_mono_16k(&fixture("en.wav"));
    let probabilities: Vec<f32> = samples
        .as_chunks::<VAD_FRAME_SAMPLES>()
        .0
        .iter()
        .map(|frame| vad.speech_probability(frame).unwrap())
        .collect();

    let speech = probabilities.iter().filter(|&&p| p > 0.5).count();
    let quiet = probabilities.iter().filter(|&&p| p < 0.2).count();
    let total = probabilities.len();
    assert!(
        speech as f32 / total as f32 > 0.3,
        "only {speech}/{total} frames detected as speech"
    );
    assert!(
        quiet > 0,
        "no frames scored as silence; the VAD cannot tell pauses apart"
    );

    let mut endpointer = Endpointer::new();
    let endpoints = probabilities
        .iter()
        .filter(|&&p| endpointer.push_frame(32, p > 0.5, Completeness::Complete).is_some())
        .count();
    assert!(endpoints >= 1, "the endpointer never closed a turn on real speech");
}
