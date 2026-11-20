//! End-to-end speaker check with the pinned WeSpeaker ResNet34 model.
//!
//! Real speech from the upstream `en.wav` fixture is split into two halves. Both halves come from the same speaker, so
//! they must score above the gate threshold. The assistant's own Kokoro voice is a different speaker, and it must score
//! below the threshold. A two-way split of one recording is weaker evidence than two recordings, and the test records
//! that. Use a fixed threshold, never one fitted to the candidates under test.

use std::path::PathBuf;

mod common;
#[path = "common/kokoro_release.rs"]
mod kokoro_release;

use eon_voice::resample::{CaptureFrames, resample_mono};
use eon_voice::speaker::{SpeakerEmbedder, SpeakerGate, cosine};
use eon_voice::tts::{KokoroTts, SAMPLE_RATE_HZ as TTS_RATE};
use eon_voice::{
    TaskId,
    barge_in::BargeIn,
    coordinator::{BargeInCoordinator, DuplexReflex},
    ring::pcm_ring,
    vad::SileroVad,
};

fn models() -> PathBuf {
    if let Some(path) = std::env::var_os("EON_VOICE_MODELS") {
        return path.into();
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../perception/models")
}

fn fixture() -> Vec<f32> {
    common::read_pcm16_mono_16k(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/en.wav"))
}

fn owner_recording() -> Vec<f32> {
    // Alternate already-authorized local recording, e.g. test_wavs/1.wav from the
    // digest-pinned LibriSpeech Zipformer archive. Never enroll from a live microphone.
    std::env::var_os("EON_VOICE_TEST_OWNER_WAV")
        .map_or_else(fixture, |path| common::read_pcm16_mono_16k(&PathBuf::from(path)))
}

#[test]
fn recorded_capture_and_render_rings_make_one_auditable_owner_decision() {
    let mut speaker = SpeakerEmbedder::load(models().join("speaker/wespeaker_resnet34.onnx")).unwrap();
    let speech = owner_recording();
    let half = speech.len() / 2;
    let gate = SpeakerGate::enroll(&speaker.embed(&speech[..half]).unwrap(), 0.35).unwrap();
    let task = TaskId::new();
    let signal = BargeIn::new();
    let coordinator = BargeInCoordinator::new(task.clone(), signal.clone(), gate);
    let audit = coordinator.audit();
    let vad = SileroVad::load(models().join("vad/silero_vad.onnx")).unwrap();
    let mut reflex = DuplexReflex::new(coordinator, vad, speaker, Some(0)).unwrap();
    let (mut playback, mut sink) = pcm_ring(480);
    let (mut capture, mic_rx) = pcm_ring(2048);
    let (mut reference, ref_rx) = pcm_ring(2048);
    let mut mic_frames = CaptureFrames::new(mic_rx, 48_000).unwrap();
    let mut ref_frames = CaptureFrames::new(ref_rx, 48_000).unwrap();
    let other = std::env::var_os("EON_ZIPFORMER_TEST_WAV_0")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/0.wav"));
    let render: Vec<_> = (0..480).map(|i| (i as f32 * std::f32::consts::TAU / 96.0).sin() * 0.1).collect();
    let mut decisions = 0;
    let mut verified_at = None;
    let non_owner = common::read_pcm16_mono_16k(&other);
    for (offset, near) in [(0, &speech[half..]), (3, &non_owner[..]), (7, &speech[half..])] {
        let near = resample_mono(near, 16_000, 48_000).unwrap();
        for (i, block) in near.as_chunks::<480>().0.iter().enumerate() {
            playback.push_slice(&render);
            let mut out = [0.0; 480];
            signal.render(&mut sink, &mut out);
            if offset == 0 {
                // A backlog of owner mic frames from *before* playback. The live
                // signal says PLAYING, but the corresponding played reference was silent.
                out.fill(0.0);
            }
            let mic: [f32; 480] = std::array::from_fn(|i| {
                (block[i] + 0.25 * out[i]).clamp(-1.0, 1.0)
            });
            assert_eq!(capture.push_slice(&mic), 480);
            assert!(reflex.poll_capture(&mut mic_frames, &mut ref_frames, offset).unwrap().is_none());
            assert_eq!(reference.push_slice(&out), 480);
            if let Some(result) = reflex.poll_capture(&mut mic_frames, &mut ref_frames, offset).unwrap() {
                decisions += usize::from(result.interrupted);
                if result.interrupted {
                    verified_at = Some((i + 1) * 10);
                }
            }
            if offset != 7 {
                assert!(audit.record().is_none(), "stale capture or non-owner interrupted playback");
            }
        }
        if offset == 3 {
            assert!(reflex.rejected_candidates() > 0, "non-owner never reached the speaker gate");
            eprintln!("recorded non-owner: {} candidate embeddings rejected", reflex.rejected_candidates());
        }
    }
    assert_eq!(decisions, 1);
    assert!(signal.is_muted());
    eprintln!("recorded owner: verified after {} ms of fixture audio", verified_at.unwrap());
    drop(reflex);
    let mut journal = Vec::new();
    assert!(audit.append_if_present(&mut journal).unwrap());
    assert!(audit.append_if_present(&mut journal).unwrap());
    let event: serde_json::Value = serde_json::from_slice(&journal).unwrap();
    assert_eq!(event["event"], "INTERRUPTED_BY_USER");
    assert_eq!(event["token_offset"], 7);
    assert_eq!(event["task_id"], task.as_str());
    assert!(event["timestamp_ms"].as_u64().unwrap() > 0);
}

/// Route the default output to a *private virtual sink* and the default input to its monitor.
/// A second CPAL stream injects released speech into that mixer, not into the AEC reference.
#[test]
#[ignore = "requires an explicitly configured virtual monitor; never run against an ambient microphone"]
fn virtual_cpal_capture_verifies_speech_and_mutes_the_reply() {
    use eon_voice::coordinator::DeviceDuplex;
    use eon_voice::device::play_default_output;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    assert_eq!(
        std::env::var("EON_VOICE_TEST_VIRTUAL_MONITOR").as_deref(),
        Ok("1"),
        "opt in only after configuring local virtual loopback"
    );
    let speech = owner_recording();
    let half = speech.len() / 2;
    let mut speaker = SpeakerEmbedder::load(models().join("speaker/wespeaker_resnet34.onnx")).unwrap();
    let owner = SpeakerGate::enroll(&speaker.embed(&speech[..half]).unwrap(), 0.35).unwrap();
    let vad = SileroVad::load(models().join("vad/silero_vad.onnx")).unwrap();
    let task = TaskId::new();
    let mut duplex = DeviceDuplex::open_default(task.clone(), owner, vad, speaker, Some(0)).unwrap();
    let audit = duplex.audit();
    let rate = duplex.output.sample_rate;
    let (near_tx, near_rx) = {
        let near = resample_mono(&speech[half..], 16_000, rate).unwrap();
        let (mut tx, rx) = pcm_ring(near.len());
        assert_eq!(tx.push_slice(&near), near.len());
        (tx, rx)
    };
    let mut journal = Vec::new();
    assert!(!audit.append_if_present(&mut journal).unwrap());
    // Keep real reply samples playing during verification. The pure tone is not a TTS benchmark.
    let period: Vec<_> = (0..rate / 100)
        .map(|i| (i as f32 * std::f32::consts::TAU * 400.0 / rate as f32).sin() * 0.04)
        .collect();
    while duplex.playback_pcm.push_slice(&period) == period.len() {}
    let start = Instant::now();
    let source = play_default_output(near_rx, BargeIn::new()).unwrap();
    let mut processed = 0;
    while start.elapsed() < Duration::from_secs(8) {
        match duplex.poll(17).unwrap() {
            Some(block) => {
                processed += 1;
                if block.interrupted {
                    break;
                }
                duplex.playback_pcm.push_slice(&period);
            }
            None => std::thread::sleep(Duration::from_micros(250)),
        }
    }
    let decision = start.elapsed();
    let request = audit.decision_instant().expect("expected an owner decision");
    while duplex.output.counters.barge_in_flushes.load(Ordering::Acquire) == 0 && request.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_micros(100));
    }
    let mute = request.elapsed();
    assert_eq!(duplex.output.counters.barge_in_flushes.load(Ordering::Acquire), 1);
    assert_eq!(duplex.input.counters.capture_drops.load(Ordering::Acquire), 0);
    assert_eq!(duplex.output.counters.reference_drops.load(Ordering::Acquire), 0);
    assert_eq!(source.counters.driver_errors.load(Ordering::Acquire), 0);
    assert!(audit.append_if_present(&mut journal).unwrap());
    let record: serde_json::Value = serde_json::from_slice(&journal).unwrap();
    assert_eq!(record["event"], "INTERRUPTED_BY_USER");
    assert_eq!(record["task_id"], task.as_str());
    assert_eq!(record["token_offset"], 17);
    eprintln!(
        "virtual CPAL input={} output={rate} Hz: {processed} paired blocks, decision {decision:?} from fixture stream launch, decision to observed callback mute <= {mute:?}",
        duplex.input.sample_rate
    );
    drop((source, near_tx));
}

#[test]
fn same_speaker_scores_above_the_gate_and_the_assistant_voice_below_it() {
    let tts = KokoroTts::load(
        models().join("tts/kokoro-v1.0-int8.onnx"),
        models().join("tts/voices/af_heart.bin"),
    )
    .expect("pinned Kokoro model must load");
    check_assistant_voice(tts);
}

#[test]
#[ignore = "requires explicitly selected and hash-checked upstream release graph; not the pinned graph test"]
fn alternate_release_voice_does_not_interrupt_and_held_out_owner_does() {
    check_assistant_voice(kokoro_release::load(&models()));
}

fn check_assistant_voice(mut tts: KokoroTts) {
    let mut embedder = SpeakerEmbedder::load(models().join("speaker/wespeaker_resnet34.onnx"))
        .expect("pinned WeSpeaker model must load");
    let speech = owner_recording();
    let half = speech.len() / 2;
    let owner = embedder.embed(&speech[..half]).unwrap();
    let same_speaker = embedder.embed(&speech[half..]).unwrap();

    let assistant = tts
        .synthesize(
            "Affirmative. I am running the test now, and the report follows shortly.",
            1.0,
        )
        .unwrap();
    let assistant_16k = resample_mono(&assistant, TTS_RATE, 16_000).unwrap();
    let other_speaker = embedder.embed(&assistant_16k).unwrap();

    let same = cosine(&owner, &same_speaker);
    let other = cosine(&owner, &other_speaker);
    eprintln!("SPEAKER same={same:.3} assistant={other:.3}");
    assert!(
        same > other + 0.2,
        "same speaker {same:.3} must clearly exceed the assistant voice {other:.3}"
    );

    let gate = SpeakerGate::enroll(&owner, 0.35).unwrap();
    assert!(gate.accepts(&same_speaker));
    assert!(!gate.accepts(&other_speaker));

    // Real model-to-render integration, not a device latency measurement. The
    // non-owner here is an independent near-end voice, not just removable echo.
    let signal = BargeIn::new();
    let task = TaskId::new();
    let coordinator = BargeInCoordinator::new(task.clone(), signal.clone(), gate);
    let vad = SileroVad::load(models().join("vad/silero_vad.onnx")).unwrap();
    let mut reflex = DuplexReflex::new(coordinator, vad, embedder, Some(45)).unwrap();
    let (mut tx, mut rx) = pcm_ring(19_200 + assistant_16k.len() + speech.len());
    // Use real TTS as the played reference; cover the entire candidate window.
    while tx.push_slice(&assistant_16k) == assistant_16k.len() {}
    let mut out = [0.0; 160];
    let mut journal = Vec::new();
    let mut echo = eon_voice::delay::DelayLine::new(720);
    let silence = [0.0; 160];
    for frame in std::iter::repeat_n(&silence, 120).chain(assistant_16k.as_chunks::<160>().0) {
        assert_eq!(signal.render(&mut rx, &mut out).samples, 160);
        let mut mic = out;
        echo.process_block(&mut mic);
        for (s, near) in mic.iter_mut().zip(frame) {
            *s = 0.5 * *s + near;
        }
        assert!(!reflex.process(&out, &mut mic, 3, &mut journal).unwrap());
    }
    assert!(journal.is_empty());
    let mut interrupted = false;
    for frame in speech[half..].as_chunks::<160>().0 {
        assert_eq!(signal.render(&mut rx, &mut out).samples, 160);
        let mut mic = out;
        echo.process_block(&mut mic);
        for (s, near) in mic.iter_mut().zip(frame) {
            *s = 0.5 * *s + near;
        }
        if reflex.process(&out, &mut mic, 7, &mut journal).unwrap() {
            interrupted = true;
            break;
        }
    }
    assert!(
        interrupted,
        "sustained verified near-end speech must cancel TTS ({} rejected candidates)",
        reflex.rejected_candidates()
    );
    let result = signal.render(&mut rx, &mut out);
    assert!(result.acknowledged && result.flushed > 0);
    assert_eq!(out, [0.0; 160]);
    let event: serde_json::Value = serde_json::from_slice(&journal).unwrap();
    assert_eq!(event["event"], "INTERRUPTED_BY_USER");
    assert_eq!(event["task_id"], task.as_str());
    assert_eq!(event["token_offset"], 7);
}
