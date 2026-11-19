//! Per-reply reflex on the perception thread. Feed 512-sample frames from the
//! echo-cleaned 16 kHz microphone stream, never from the audio callback.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use eon_primitives::{EonError, EonResult, TaskId};

use crate::aec::AcousticEchoCanceller;
use crate::barge_in::BargeIn;
use crate::device::{PlaybackHandle, play_default_output_with_reference};
use crate::geometry::{AEC_BLOCK_SAMPLES, SAMPLE_RATE_HZ, VAD_FRAME_SAMPLES};
use crate::linear_echo::NlmsCanceller;
use crate::ring::{PcmConsumer, PcmProducer};
use crate::speaker::{SpeakerEmbedder, SpeakerGate};
use crate::vad::SileroVad;

// WeSpeaker needs at least 50 fbank frames. Sixteen Silero frames give it 512 ms.
const CANDIDATE_SAMPLES: usize = 16 * VAD_FRAME_SAMPLES;

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
}

impl DuplexReflex {
    pub fn new(
        coordinator: BargeInCoordinator,
        vad: SileroVad,
        speaker: SpeakerEmbedder,
        delay_ms: Option<u16>,
    ) -> EonResult<Self> {
        Ok(Self {
            aec: AcousticEchoCanceller::new(SAMPLE_RATE_HZ, delay_ms)?,
            linear: NlmsCanceller::new(1024, 0.2),
            vad,
            speaker,
            coordinator,
            frame: [0.0; VAD_FRAME_SAMPLES],
            filled: 0,
        })
    }

    pub fn interruption(&self) -> Option<&Interruption> {
        self.coordinator.interruption()
    }

    /// Cleans `capture` in place with AEC3 for downstream ASR. Verification uses
    /// the linear residual so the nonlinear suppressor can't erase double-talk.
    pub fn process(
        &mut self,
        render: &[f32; AEC_BLOCK_SAMPLES],
        capture: &mut [f32; AEC_BLOCK_SAMPLES],
        token_offset: usize,
        journal: &mut impl Write,
    ) -> EonResult<bool> {
        self.aec.analyze_render(render)?;
        let mut interrupted = false;
        for (&reference, &mic) in render.iter().zip(capture.iter()) {
            self.linear.push_render(reference);
            self.frame[self.filled] = self.linear.process(mic);
            self.filled += 1;
            if self.filled == self.frame.len() {
                self.filled = 0;
                interrupted |= self.coordinator.observe_capture(
                    &self.frame,
                    &mut self.vad,
                    &mut self.speaker,
                    token_offset,
                    journal,
                )?;
            }
        }
        self.aec.process_capture(capture)?;
        Ok(interrupted)
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

pub struct BargeInCoordinator {
    playback: BargeIn,
    task: TaskId,
    owner: SpeakerGate,
    voiced: Vec<f32>,
    interrupted: Option<Interruption>,
}

impl BargeInCoordinator {
    /// Opens native playback with this coordinator's cancellation signal and a
    /// post-render reference ring for the perception worker.
    pub fn start_default_output(
        task: TaskId,
        owner: SpeakerGate,
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
    pub fn new(task: TaskId, playback: BargeIn, owner: SpeakerGate) -> Self {
        Self {
            playback,
            task,
            owner,
            voiced: Vec::with_capacity(CANDIDATE_SAMPLES),
            interrupted: None,
        }
    }

    /// Remains available even when appending the journal record failed.
    pub fn interruption(&self) -> Option<&Interruption> {
        self.interrupted.as_ref()
    }

    /// Verifies sustained Silero speech using a real WeSpeaker embedding of those same
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
        if !self.playback.is_playing() {
            self.voiced.clear();
            return Ok(false);
        }
        match vad.speech_probability(frame) {
            Ok(p) if (0.5..=1.0).contains(&p) => self.voiced.extend_from_slice(frame),
            result => {
                self.voiced.clear();
                return result.map(|_| false);
            }
        }
        if self.voiced.len() < CANDIDATE_SAMPLES {
            return Ok(false);
        }
        let candidate = speaker.embed(&self.voiced);
        self.voiced.clear();
        self.on_candidate(&candidate?, token_offset, journal)
    }

    fn on_candidate(&mut self, candidate: &[f32], token_offset: usize, journal: &mut impl Write) -> EonResult<bool> {
        if !self.owner.accepts(candidate) || !self.playback.interrupt_playing() {
            return Ok(false);
        }
        let event = self.interrupted.insert(Interruption {
            task_id: self.task.clone(),
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            token_offset,
        });
        event.write_to(journal).map_err(|e| EonError::OperationFailed {
            operation: "voice.record_interruption".into(),
            reason: e.to_string(),
        })?;
        Ok(true)
    }
}

#[cfg(test)]
mod barge_in_tests {
    use super::*;

    #[test]
    fn a_full_journal_cannot_delay_or_undo_mute() {
        let candidate = [1.0; EMBEDDING_DIM];
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
        assert!(coordinator.on_candidate(&candidate, 5, &mut full).is_err());
        assert_eq!(coordinator.interruption().unwrap().token_offset, 5);
        assert!(signal.render(&mut rx, &mut [0.0; 2]).muted);
    }
    use crate::{ring::pcm_ring, speaker::EMBEDDING_DIM};

    // Decision/render boundary only; this does not test either neural model.
    #[test]
    fn only_a_matching_candidate_during_playback_mutes_and_records_once() {
        let mut owner = [0.0; EMBEDDING_DIM];
        owner[0] = 1.0;
        let task = TaskId::new();
        let signal = BargeIn::new();
        let mut coordinator =
            BargeInCoordinator::new(task.clone(), signal.clone(), SpeakerGate::enroll(&owner, 0.5).unwrap());
        let (mut tx, mut rx) = pcm_ring(16);
        let mut out = [0.0; 4];
        let mut journal = Vec::new();
        assert!(!coordinator.on_candidate(&owner, 0, &mut journal).unwrap());
        tx.push_slice(&[0.4; 16]);
        signal.render(&mut rx, &mut out);
        assert_eq!(out, [0.4; 4]);
        let mut other = [0.0; EMBEDDING_DIM];
        other[1] = 1.0;
        for invalid in [&other[..], &owner[..1], &[f32::NAN; EMBEDDING_DIM]] {
            assert!(!coordinator.on_candidate(invalid, 2, &mut journal).unwrap());
        }
        assert!(journal.is_empty());
        assert!(coordinator.on_candidate(&owner, 7, &mut journal).unwrap());
        let result = signal.render(&mut rx, &mut out);
        assert!(result.acknowledged);
        assert_eq!(result.flushed, 12);
        assert_eq!(out, [0.0; 4]);
        tx.push_slice(&[0.9; 6]); // synthesis already in flight
        let result = signal.render(&mut rx, &mut out);
        assert!(result.muted && !result.acknowledged);
        assert_eq!(rx.available(), 0);
        assert_eq!(out, [0.0; 4]);
        assert!(!coordinator.on_candidate(&owner, 8, &mut journal).unwrap());
        let record: serde_json::Value = serde_json::from_slice(&journal).unwrap();
        assert_eq!(record["event"], "INTERRUPTED_BY_USER");
        assert_eq!(record["task_id"], task.as_str());
        assert_eq!(record["token_offset"], 7);
        assert!(record["timestamp_ms"].as_u64().unwrap() > 0);
    }
}
