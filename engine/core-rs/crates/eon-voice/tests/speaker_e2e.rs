//! End-to-end speaker check with the pinned WeSpeaker ResNet34 model.
//!
//! Real speech from the upstream `en.wav` fixture is split into two halves. Both halves come from the same speaker, so
//! they must score above the gate threshold. The assistant's own Kokoro voice is a different speaker, and it must score
//! below the threshold. Exercise both delayed echo and independent near-end speech through the duplex reflex.
//! A split of one recording is weaker evidence than two recordings; two competing voices aren't a crowd corpus.

use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod common;
#[path = "common/kokoro_release.rs"]
mod kokoro_release;
#[cfg(target_os = "linux")]
#[path = "speaker/kokoro_stop.rs"]
mod kokoro_stop;
#[cfg(target_os = "linux")]
#[path = "common/private_monitor.rs"]
mod private_monitor;

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
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an explicitly configured virtual monitor; never run against an ambient microphone"]
fn virtual_cpal_capture_verifies_speech_and_mutes_the_reply() {
    use eon_voice::coordinator::DeviceDuplex;
    use eon_voice::device::play_default_output;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    private_monitor::require_null_sink();
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
    time_verification(&mut embedder, &speech[half..], &owner);
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
    let mut echo_rejected = 0;
    for (i, frame) in std::iter::repeat_n(&silence, 120)
        .chain(assistant_16k.as_chunks::<160>().0)
        .enumerate()
    {
        assert_eq!(signal.render(&mut rx, &mut out).samples, 160);
        let mut mic = out;
        echo.process_block(&mut mic);
        for (s, near) in mic.iter_mut().zip(frame) {
            *s = 0.5 * *s + near;
        }
        assert!(!reflex.process(&out, &mut mic, 3, &mut journal).unwrap());
        if i == 119 {
            echo_rejected = reflex.rejected_candidates();
        }
    }
    assert!(journal.is_empty());
    let assistant_rejected = reflex.rejected_candidates() - echo_rejected;
    assert!(assistant_rejected > 0, "assistant near-end voice never reached verification");
    let mut owner_ms = None;
    for (i, frame) in speech[half..].as_chunks::<160>().0.iter().enumerate() {
        assert_eq!(signal.render(&mut rx, &mut out).samples, 160);
        let mut mic = out;
        echo.process_block(&mut mic);
        for (s, near) in mic.iter_mut().zip(frame) {
            *s = 0.5 * *s + near;
        }
        if reflex.process(&out, &mut mic, 7, &mut journal).unwrap() {
            owner_ms = Some((i + 1) * 10);
            break;
        }
    }
    assert!(
        owner_ms.is_some(),
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
    eprintln!(
        "45 ms echo: {echo_rejected} echo-only and {assistant_rejected} assistant candidates rejected, \
         owner interrupted once after {} ms of held-out audio",
        owner_ms.unwrap()
    );

    // A second scene, not a phase-shifted copy of playback: an independent
    // utterance overlaps the released 0.wav recording outside the reference.
    let competing = tts
        .synthesize("We walked through the garden and watched the birds in the trees.", 1.0)
        .unwrap();
    check_competing_speech(&assistant, &competing, &owner, &speech[half..]);
}

fn time_verification(speaker: &mut SpeakerEmbedder, held_out: &[f32], owner: &[f32]) {
    // Warm inference at the actual gate window size. Includes fbank, CMN,
    // ONNX and normalization; excludes model load and waiting for 512 ms of PCM.
    let pcm = &held_out[..8192];
    let candidate = speaker.embed(pcm).unwrap();
    let mut embed_times = [Duration::ZERO; 9];
    for elapsed in &mut embed_times {
        let start = Instant::now();
        black_box(speaker.embed(black_box(pcm)).unwrap());
        *elapsed = start.elapsed();
    }
    let mut cosine_times = [Duration::ZERO; 21];
    for elapsed in &mut cosine_times {
        let start = Instant::now();
        for _ in 0..1000 {
            black_box(cosine(black_box(owner), black_box(&candidate)));
        }
        *elapsed = start.elapsed() / 1000;
    }
    cosine_times.sort_unstable();
    embed_times.sort_unstable();
    eprintln!(
        "256-d cosine (21 x 1000, per-call batch mean) median {:?}, p95 {:?}; \
         512 ms WeSpeaker embed (9 warm calls) median {:?}, max {:?}",
        cosine_times[10], cosine_times[19], embed_times[4], embed_times[8],
    );
    assert!(cosine_times[19] < Duration::from_millis(2));
}

fn power(pcm: &[f32]) -> f32 {
    pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len() as f32
}

fn check_competing_speech(reply: &[f32], competing: &[f32], owner: &[f32], held_out: &[f32]) {
    use eon_voice::delay::DelayLine;
    // All three streams start on the same 24 kHz sample clock. Both device
    // rings use the production 24 -> 16 kHz readers. Nothing opens a device.
    const BLOCK: usize = TTS_RATE as usize / 100;
    let wav = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/0.wav");
    let recorded = resample_mono(&common::read_pcm16_mono_16k(&wav), 16_000, TTS_RATE).unwrap();
    let held_out = resample_mono(held_out, 16_000, TTS_RATE).unwrap();
    // Balance the two near-end sources by whole-clip RMS, not gate scores.
    let gain = (power(&recorded) / power(competing)).sqrt();
    let competing: Vec<_> = competing.iter().cycle().take(recorded.len()).map(|x| x * gain).collect();
    let mixture: Vec<_> = recorded.iter().zip(&competing).map(|(x, y)| 0.5 * (x + y)).collect();
    let silence = vec![0.0; reply.len()];
    let total = silence.len() + mixture.len() + held_out.len() + BLOCK;
    let looped: Vec<_> = reply.iter().copied().cycle().take(total).collect();
    let (mut playback, mut sink) = pcm_ring(total);
    assert_eq!(playback.push_slice(&looped), total);
    let (mut capture, mic_rx) = pcm_ring(4 * BLOCK);
    let (mut reference, ref_rx) = pcm_ring(4 * BLOCK);
    let mut mic_frames = CaptureFrames::new(mic_rx, TTS_RATE).unwrap();
    let mut ref_frames = CaptureFrames::new(ref_rx, TTS_RATE).unwrap();
    let signal = BargeIn::new();
    let task = TaskId::new();
    let coordinator = BargeInCoordinator::new(task.clone(), signal.clone(), SpeakerGate::enroll(owner, 0.35).unwrap());
    let audit = coordinator.audit();
    let vad = SileroVad::load(models().join("vad/silero_vad.onnx")).unwrap();
    let speaker = SpeakerEmbedder::load(models().join("speaker/wespeaker_resnet34.onnx")).unwrap();
    let mut reflex = DuplexReflex::new(coordinator, vad, speaker, Some(30)).unwrap();
    // Different delay and a second reflection, fixed before running this scene.
    let mut direct = DelayLine::new(3 * BLOCK);
    let mut reflected = DelayLine::new(5 * BLOCK);
    let mut out = [0.0; BLOCK];
    let mut simultaneous_ms = 0;
    let mut owner_ms = None;
    for (phase, near) in [silence, mixture, held_out].iter().enumerate() {
        let before = reflex.rejected_candidates();
        let mut active_playback = false;
        for (i, block) in near.as_chunks::<BLOCK>().0.iter().enumerate() {
            assert_eq!(signal.render(&mut sink, &mut out).samples, BLOCK);
            let active = power(&out) > 1e-4; // -40 dBFS per 10 ms
            active_playback |= active;
            let range = i * BLOCK..(i + 1) * BLOCK;
            if phase == 1 && active && power(&recorded[range.clone()]) > 1e-4 && power(&competing[range]) > 1e-4 {
                simultaneous_ms += 10;
            }
            let mic: [f32; BLOCK] = std::array::from_fn(|j| {
                (block[j] + 0.4 * direct.process(out[j]) - 0.15 * reflected.process(out[j])).clamp(-1.0, 1.0)
            });
            assert_eq!(reference.push_slice(&out), BLOCK);
            assert_eq!(capture.push_slice(&mic), BLOCK);
            if reflex
                .poll_capture(&mut mic_frames, &mut ref_frames, phase)
                .unwrap()
                .is_some_and(|b| b.interrupted)
            {
                assert_eq!(phase, 2, "echo/competing voices interrupted");
                owner_ms = Some((i + 1) * 10);
                break;
            }
        }
        assert!(active_playback);
        if phase < 2 {
            let rejected = reflex.rejected_candidates() - before;
            assert!(audit.record().is_none() && !signal.is_muted());
            eprintln!("24 kHz phase {phase}: {rejected} candidate rejections, no interruption");
            if phase == 1 {
                assert!(simultaneous_ms >= 512 && rejected >= 2, "insufficient competing speech verification");
            }
        }
    }
    let owner_ms = owner_ms.expect("held-out owner must interrupt");
    let muted = signal.render(&mut sink, &mut out);
    assert!(muted.acknowledged && muted.flushed > 0 && out == [0.0; BLOCK]);
    assert!(!signal.render(&mut sink, &mut out).acknowledged);
    let event = audit.record().unwrap();
    assert_eq!(event.task_id, task);
    assert_eq!(event.token_offset, 2);
    eprintln!("24 kHz: {simultaneous_ms} ms three-way active; owner interrupted once after {owner_ms} ms");
}
