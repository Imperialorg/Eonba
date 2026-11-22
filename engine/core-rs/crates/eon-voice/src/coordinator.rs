//! Per-reply reflex on the perception thread. Feed 512-sample frames from the
//! echo-cleaned 16 kHz microphone stream, never from the audio callback.

use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use eon_primitives::{EonError, EonResult, TaskId};

use crate::aec::AcousticEchoCanceller;
use crate::barge_in::BargeIn;
use crate::device::{CaptureHandle, PlaybackHandle, capture_default_frames, play_default_output_with_reference};
use crate::geometry::{AEC_BLOCK_SAMPLES, SAMPLE_RATE_HZ, VAD_FRAME_SAMPLES};
use crate::linear_echo::NlmsCanceller;
use crate::resample::CaptureFrames;
use crate::ring::{PcmConsumer, PcmProducer, pcm_ring};
use crate::speaker::{SpeakerEmbedder, SpeakerEmbedding, SpeakerPolicy};
use crate::speaker::windows::SpeakerWindows;
use crate::vad::SileroVad;

pub use crate::speaker::windows::CANDIDATE_SAMPLES;

/// Capture-side wiring for the two 16 kHz, time-aligned streams. The render input
/// must be the resampled *played* reference, including silence, not pending TTS.
pub struct DuplexReflex {
    aec: AcousticEchoCanceller,
    linear: NlmsCanceller,
    vad: SileroVad,
    speaker: SpeakerEmbedder,
    coordinator: BargeInCoordinator,
    frame: [f32; VAD_FRAME_SAMPLES],
    filled: usize,
    frame_has_render: bool,
}

impl DuplexReflex {
    pub fn new(
        coordinator: BargeInCoordinator,
        vad: SileroVad,
        speaker: SpeakerEmbedder,
        delay_ms: Option<u16>,
    ) -> EonResult<Self> {
        coordinator.check_encoder(&speaker)?;
        Ok(Self {
            aec: AcousticEchoCanceller::new(SAMPLE_RATE_HZ, delay_ms)?,
            linear: NlmsCanceller::new(1024, 0.2),
            vad,
            speaker,
            coordinator,
            frame: [0.0; VAD_FRAME_SAMPLES],
            filled: 0,
            frame_has_render: false,
        })
    }

    pub fn interruption(&self) -> Option<&Interruption> {
        self.coordinator.interruption()
    }

    /// Completed embeddings rejected by the owner gate (not merely frames rejected by VAD).
    pub fn rejected_candidates(&self) -> u64 {
        self.coordinator.rejected_candidates
    }

    /// Poll one pair, without draining either side ahead of the other. The rings must
    /// share a start time; this converts rates, it does not estimate hardware clock drift.
    /// On any processing error retire both streams rather than splicing across the gap.
    pub fn poll_capture(
        &mut self,
        capture: &mut CaptureFrames<AEC_BLOCK_SAMPLES>,
        reference: &mut CaptureFrames<AEC_BLOCK_SAMPLES>,
        token_offset: usize,
    ) -> EonResult<Option<DuplexBlock>> {
        let capture_ready = capture.ready()?;
        let reference_ready = reference.ready()?;
        if !capture_ready || !reference_ready {
            return Ok(None);
        }
        // Only this worker owns the consumers; callbacks can only add samples.
        let render = reference.try_read()?.expect("complete render block");
        let mut pcm = capture.try_read()?.expect("complete capture block");
        let interrupted = self.process_block(&render, &mut pcm, token_offset)?;
        Ok(Some(DuplexBlock { pcm, interrupted }))
    }

    fn process_block(
        &mut self,
        render: &[f32; AEC_BLOCK_SAMPLES],
        capture: &mut [f32; AEC_BLOCK_SAMPLES],
        token_offset: usize,
    ) -> EonResult<bool> {
        let mic = *capture;
        self.aec.analyze_render(render)?;
        self.aec.process_capture(capture)?;
        let mut interrupted = false;
        for (&reference, mic) in render.iter().zip(mic) {
            self.linear.push_render(reference);
            self.frame_has_render |= reference.abs() > f32::EPSILON;
            self.frame[self.filled] = self.linear.process(mic).clamp(-1.0, 1.0);
            self.filled += 1;
            if self.filled == self.frame.len() {
                self.filled = 0;
                // The worker may be behind the callback: PLAYING *now* is not evidence
                // that a queued utterance overlapped any played reply at capture time.
                if std::mem::take(&mut self.frame_has_render) {
                    interrupted |=
                        self.coordinator
                            .observe(&self.frame, &mut self.vad, &mut self.speaker, token_offset)?;
                } else {
                    self.coordinator.reset_candidate(&mut self.vad);
                }
            }
        }
        Ok(interrupted)
    }

    /// Synchronous journal adapter for caller-supplied aligned 16 kHz blocks.
    /// `poll_capture` instead publishes the same audit record without calling a writer.
    pub fn process(
        &mut self,
        render: &[f32; AEC_BLOCK_SAMPLES],
        capture: &mut [f32; AEC_BLOCK_SAMPLES],
        token_offset: usize,
        journal: &mut impl Write,
    ) -> EonResult<bool> {
        let interrupted = self.process_block(render, capture, token_offset)?;
        if interrupted {
            self.coordinator.persist(journal)?;
        }
        Ok(interrupted)
    }
}

pub struct DuplexBlock {
    /// Echo-cleaned, 16 kHz PCM for local downstream ASR.
    pub pcm: [f32; AEC_BLOCK_SAMPLES],
    pub interrupted: bool,
}

/// Opt-in, reply-scoped device endpoints. No worker thread, recording or journal file
/// is opened implicitly. Poll on the perception thread, never the CPAL callback.
pub struct DeviceDuplex {
    pub output: PlaybackHandle,
    pub input: CaptureHandle,
    /// Enqueue reply PCM at `output.sample_rate`.
    pub playback_pcm: PcmProducer,
    reflex: DuplexReflex,
    capture: CaptureFrames<AEC_BLOCK_SAMPLES>,
    reference: CaptureFrames<AEC_BLOCK_SAMPLES>,
}

impl DeviceDuplex {
    pub fn open_default(
        task: TaskId,
        owner: impl Into<SpeakerPolicy>,
        vad: SileroVad,
        speaker: SpeakerEmbedder,
        delay_ms: Option<u16>,
    ) -> EonResult<Self> {
        let (playback_pcm, rx) = pcm_ring(1 << 15);
        let (tx, reference) = pcm_ring(1 << 15);
        let (coordinator, output) = BargeInCoordinator::start_default_output(task, owner, rx, tx)?;
        let reflex = DuplexReflex::new(coordinator, vad, speaker, delay_ms)?;
        let mut reference = CaptureFrames::new(reference, output.sample_rate)?;
        let (mut capture, input) = capture_default_frames()?;
        input.wait_for_input()?;
        // Establish a common software origin after *both* streams have delivered data.
        // This excludes output-only startup silence, not hardware latency or drift.
        capture.discard_startup();
        reference.discard_startup();
        Ok(Self {
            output,
            input,
            playback_pcm,
            reflex,
            capture,
            reference,
        })
    }

    /// Pass to the journal owner, which can read the single published record even after
    /// capture stops. JSON serialization and flushing belong on that thread.
    pub fn audit(&self) -> InterruptionAudit {
        self.reflex.coordinator.audit()
    }

    pub fn playback_signal(&self) -> BargeIn {
        self.reflex.coordinator.playback_signal()
    }

    /// Poll frequently; an overflow or driver failure is an error, not permission
    /// to verify non-contiguous audio.
    pub fn poll(&mut self, token_offset: usize) -> EonResult<Option<DuplexBlock>> {
        if self.input.counters.driver_errors.load(Ordering::Acquire) != 0
            || self.output.counters.driver_errors.load(Ordering::Acquire) != 0
        {
            return Err(EonError::OperationFailed {
                operation: "voice.duplex".into(),
                reason: "device stream failed; restart the reply streams".into(),
            });
        }
        self.reflex
            .poll_capture(&mut self.capture, &mut self.reference, token_offset)
    }
}

#[derive(Debug)]
pub struct Interruption {
    pub task_id: TaskId,
    pub timestamp_ms: u64,
    pub token_offset: usize,
}

impl Interruption {
    pub const KIND: &'static str = "INTERRUPTED_BY_USER";

    pub fn write_to(&self, journal: &mut impl Write) -> std::io::Result<()> {
        let event = serde_json::json!({
            "event": Self::KIND, "task_id": self.task_id.as_str(),
            "timestamp_ms": self.timestamp_ms, "token_offset": self.token_offset,
        });
        writeln!(journal, "{event}")?;
        journal.flush()
    }
}

/// One immutable, metadata-only slot per reply, with a single publisher and nonblocking
/// readers. A slow or broken durable sink cannot stall capture or undo cancellation.
#[derive(Clone, Default)]
pub struct InterruptionAudit(Arc<AuditSlot>);

#[derive(Default)]
struct AuditSlot {
    event: OnceLock<(Instant, Interruption)>,
    append: OnceLock<std::io::Result<()>>,
}

impl InterruptionAudit {
    pub fn record(&self) -> Option<&Interruption> {
        self.0.event.get().map(|(_, record)| record)
    }

    /// Local monotonic instant immediately before requesting mute. This is not
    /// speech onset, callback acknowledgment or sound reaching a physical device.
    pub fn decision_instant(&self) -> Option<Instant> {
        self.0.event.get().map(|(instant, _)| *instant)
    }

    /// Journal/control thread only. False until there is an event; true once the single
    /// JSONL append and flush succeeded. A sink can fail *after* a partial write, so all
    /// readers retain that failure rather than automatically appending a duplicate.
    pub fn append_if_present(&self, journal: &mut impl Write) -> std::io::Result<bool> {
        let Some(event) = self.record() else {
            return Ok(false);
        };
        match self.0.append.get_or_init(|| event.write_to(journal)) {
            Ok(()) => Ok(true),
            Err(e) => Err(std::io::Error::new(e.kind(), e.to_string())),
        }
    }
}

pub struct BargeInCoordinator {
    playback: BargeIn,
    task: TaskId,
    owner: SpeakerPolicy,
    windows: SpeakerWindows,
    rejected_candidates: u64,
    audit: InterruptionAudit,
}

impl BargeInCoordinator {
    /// Opens native playback with this coordinator's cancellation signal and a
    /// post-render reference ring for the perception worker.
    pub fn start_default_output(
        task: TaskId,
        owner: impl Into<SpeakerPolicy>,
        pcm: PcmConsumer,
        reference: PcmProducer,
    ) -> EonResult<(Self, PlaybackHandle)> {
        let signal = BargeIn::new();
        let driver = play_default_output_with_reference(pcm, signal.clone(), reference)?;
        Ok((Self::new(task, signal, owner), driver))
    }

    pub fn playback_signal(&self) -> BargeIn {
        self.playback.clone()
    }

    /// `playback` must be the same reply's signal passed to the output driver.
    pub fn new(task: TaskId, playback: BargeIn, owner: impl Into<SpeakerPolicy>) -> Self {
        let owner = owner.into();
        Self {
            playback,
            task,
            windows: SpeakerWindows::new(owner.encoder()),
            owner,
            rejected_candidates: 0,
            audit: InterruptionAudit::default(),
        }
    }

    /// Remains available even when appending the journal record failed.
    pub fn interruption(&self) -> Option<&Interruption> {
        self.audit.record()
    }

    pub fn audit(&self) -> InterruptionAudit {
        self.audit.clone()
    }

    fn check_encoder(&self, speaker: &SpeakerEmbedder) -> EonResult<()> {
        if self.owner.encoder() != speaker.encoder() {
            return Err(EonError::ValidationError("enrollment and candidate speaker encoders differ".into()));
        }
        Ok(())
    }

    /// Verifies sustained Silero speech using a speaker embedding of those same
    /// frames. A successful decision latches playback mute *before* journal I/O.
    /// `token_offset` is the current reply cursor, not the capture sample position.
    pub fn observe_capture(
        &mut self,
        frame: &[f32],
        vad: &mut SileroVad,
        speaker: &mut SpeakerEmbedder,
        token_offset: usize,
        journal: &mut impl Write,
    ) -> EonResult<bool> {
        if let Err(error) = self.check_encoder(speaker) {
            self.windows.clear(); // A later retry cannot bridge the frame that was refused.
            return Err(error);
        }
        let interrupted = self.observe(frame, vad, speaker, token_offset)?;
        if interrupted {
            self.persist(journal)?;
        }
        Ok(interrupted)
    }

    fn observe(
        &mut self,
        frame: &[f32],
        vad: &mut SileroVad,
        speaker: &mut SpeakerEmbedder,
        token_offset: usize,
    ) -> EonResult<bool> {
        if !self.playback.is_playing() {
            self.reset_candidate(vad);
            return Ok(false);
        }
        match vad.speech_probability(frame) {
            Ok(p) if (0.5..=1.0).contains(&p) => (),
            result => {
                self.windows.clear();
                return result.map(|_| false);
            }
        }
        self.observe_voiced(frame, |pcm| speaker.embed(pcm), token_offset)
    }

    fn observe_voiced(
        &mut self,
        frame: &[f32],
        embed: impl FnOnce(&[f32]) -> EonResult<SpeakerEmbedding>,
        token_offset: usize,
    ) -> EonResult<bool> {
        let current = self.windows.candidate(frame, |pcm| self.owner.verify(&embed(pcm)?, pcm))?;
        Ok(current.is_some_and(|accepted| self.on_verification(accepted, token_offset)))
    }

    fn reset_candidate(&mut self, vad: &mut SileroVad) {
        self.windows.clear();
        vad.reset();
    }

    fn on_verification(&mut self, accepted: bool, token_offset: usize) -> bool {
        if !accepted {
            self.rejected_candidates = self.rejected_candidates.saturating_add(1);
            return false;
        }
        let decision = Instant::now();
        if !self.playback.interrupt_playing() {
            return false;
        }
        let event = Interruption {
            task_id: self.task.clone(),
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            token_offset,
        };
        self.audit
            .0
            .event
            .set((decision, event))
            .expect("only the winning playback transition publishes");
        true
    }

    fn persist(&self, journal: &mut impl Write) -> EonResult<()> {
        self.audit
            .append_if_present(journal)
            .map(|_| ())
            .map_err(|e| EonError::OperationFailed {
                operation: "voice.record_interruption".into(),
                reason: e.to_string(),
            })
    }
}

#[cfg(test)]
mod barge_in_tests {
    use super::*;
    use crate::{ring::pcm_ring, speaker::{EMBEDDING_DIM, SpeakerEncoder, SpeakerGate}};

    impl BargeInCoordinator {
        fn on_candidate(&mut self, candidate: &SpeakerEmbedding, token_offset: usize) -> bool {
            let accepted = self.owner.verify(candidate, &[]).unwrap();
            self.on_verification(accepted, token_offset)
        }
    }

    fn embedding(values: &[f32]) -> SpeakerEmbedding {
        SpeakerEmbedding::from_values(SpeakerEncoder::WeSpeakerResNet34LmV1, values).unwrap()
    }

    #[test]
    fn a_long_point_eight_cannot_authorize_after_zero_current_evidence() {
        let mut v = [0.; EMBEDDING_DIM];
        v[0] = 1.;
        let gate = SpeakerGate::enroll(&embedding(&v), 0.35).unwrap();
        let signal = BargeIn::new();
        let mut coordinator = BargeInCoordinator::new(TaskId::new(), signal.clone(), gate);
        let (mut tx, mut rx) = pcm_ring(8);
        tx.push_slice(&[0.2; 8]);
        signal.render(&mut rx, &mut [0.; 2]);
        for i in 1..=8 * CANDIDATE_SAMPLES / VAD_FRAME_SAMPLES {
            let interrupted = coordinator.observe_voiced(&[i as f32; VAD_FRAME_SAMPLES], |pcm| {
                // Old 4-window cosine: .8. Every short/double window: zero.
                (v[0], v[1]) = if pcm.len() == 4 * CANDIDATE_SAMPLES { (0.8, 0.6) } else { (0., 1.) };
                Ok(embedding(&v))
            }, i).unwrap();
            assert!(!interrupted, "old audio authorized a negative current speaker at frame {i}");
        }
        assert!(coordinator.rejected_candidates > 0);
        assert!(signal.is_playing() && !signal.is_muted());
        assert!(coordinator.interruption().is_none());
    }

    #[test]
    fn a_stalled_audit_flush_does_not_block_render_and_is_not_replayed_after_failure() {
        use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

        struct Sink {
            flushing: SyncSender<()>,
            resume: Receiver<()>,
        }
        impl Write for Sink {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.flushing.send(()).unwrap();
                self.resume.recv().unwrap();
                Err(std::io::Error::other("flush failed after write"))
            }
        }
        let owner = embedding(&[1.0; EMBEDDING_DIM]);
        let signal = BargeIn::new();
        let mut coordinator =
            BargeInCoordinator::new(TaskId::new(), signal.clone(), SpeakerGate::enroll(&owner, 0.5).unwrap());
        let (mut tx, mut rx) = pcm_ring(8);
        tx.push_slice(&[0.2; 8]);
        let mut out = [0.0; 2];
        signal.render(&mut rx, &mut out);
        assert!(coordinator.on_candidate(&owner, 19));
        let audit = coordinator.audit();
        let (flushing, wait) = sync_channel(0);
        let (release, resume) = sync_channel(0);
        let writer = std::thread::spawn(move || audit.append_if_present(&mut Sink { flushing, resume }));
        wait.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert!(signal.render(&mut rx, &mut out).acknowledged);
        assert_eq!(out, [0.0; 2]);
        assert_eq!(coordinator.interruption().unwrap().token_offset, 19);
        release.send(()).unwrap();
        assert!(writer.join().unwrap().is_err());
        let mut retry = Vec::new();
        assert!(coordinator.audit().append_if_present(&mut retry).is_err());
        assert!(retry.is_empty());
    }

    #[test]
    fn a_full_journal_cannot_delay_or_undo_mute() {
        let candidate = embedding(&[1.0; EMBEDDING_DIM]);
        let signal = BargeIn::new();
        let mut coordinator = BargeInCoordinator::new(
            TaskId::new(),
            signal.clone(),
            SpeakerGate::enroll(&candidate, 0.5).unwrap(),
        );
        let (mut tx, mut rx) = pcm_ring(8);
        tx.push_slice(&[0.2; 8]);
        signal.render(&mut rx, &mut [0.0; 2]);
        let mut full = &mut [][..];
        let reader = coordinator.audit();
        assert!(coordinator.on_candidate(&candidate, 5));
        assert!(coordinator.persist(&mut full).is_err());
        assert_eq!(coordinator.interruption().unwrap().token_offset, 5);
        assert!(signal.render(&mut rx, &mut [0.0; 2]).muted);
        drop(coordinator);
        assert_eq!(reader.record().unwrap().token_offset, 5);
        let mut other = Vec::new();
        assert!(reader.append_if_present(&mut other).is_err());
        assert!(other.is_empty());
    }

    // Decision/render boundary only; this does not test either neural model.
    #[test]
    fn only_a_matching_candidate_during_playback_mutes_and_records_once() {
        let mut owner = [0.0; EMBEDDING_DIM];
        owner[0] = 1.0;
        let owner = embedding(&owner);
        let task = TaskId::new();
        let signal = BargeIn::new();
        let mut coordinator =
            BargeInCoordinator::new(task.clone(), signal.clone(), SpeakerGate::enroll(&owner, 0.5).unwrap());
        let (mut tx, mut rx) = pcm_ring(16);
        let mut out = [0.0; 4];
        let mut journal = Vec::new();
        assert!(!coordinator.on_candidate(&owner, 0));
        tx.push_slice(&[0.4; 16]);
        signal.render(&mut rx, &mut out);
        assert_eq!(out, [0.4; 4]);
        let mut other = [0.0; EMBEDDING_DIM];
        other[1] = 1.0;
        assert!(!coordinator.on_candidate(&embedding(&other), 2));
        let wrong_encoder = SpeakerEmbedding::from_values(SpeakerEncoder::CamPlusPlusZhEnAdvancedV1, &[1.0; 192]).unwrap();
        assert!(!coordinator.on_candidate(&wrong_encoder, 2));
        assert!(journal.is_empty());
        assert!(coordinator.on_candidate(&owner, 7));
        coordinator.persist(&mut journal).unwrap();
        let result = signal.render(&mut rx, &mut out);
        assert!(result.acknowledged);
        assert_eq!(result.flushed, 12);
        assert_eq!(out, [0.0; 4]);
        tx.push_slice(&[0.9; 6]); // synthesis already in flight
        let result = signal.render(&mut rx, &mut out);
        assert!(result.muted && !result.acknowledged);
        assert_eq!(rx.available(), 0);
        assert_eq!(out, [0.0; 4]);
        assert!(!coordinator.on_candidate(&owner, 8));
        let record: serde_json::Value = serde_json::from_slice(&journal).unwrap();
        assert_eq!(record["event"], "INTERRUPTED_BY_USER");
        assert_eq!(record["task_id"], task.as_str());
        assert_eq!(record["token_offset"], 7);
        assert!(record["timestamp_ms"].as_u64().unwrap() > 0);
    }
}
