//! Select a candidate from uninterrupted voiced playback. The gate must still
//! verify the selected embedding at the actual playback transition.

use super::{SpeakerEmbedding, SpeakerEncoder, SpeakerGate};
use crate::geometry::VAD_FRAME_SAMPLES;
use eon_primitives::{EonError, EonResult};

/// Shortest whole number of Silero frames yielding 50 complete fbank windows.
pub const CANDIDATE_SAMPLES: usize =
    super::MIN_SAMPLES.div_ceil(VAD_FRAME_SAMPLES) * VAD_FRAME_SAMPLES;
const SHORT: usize = CANDIDATE_SAMPLES;
const OFFSET: usize = (SHORT / VAD_FRAME_SAMPLES / 2) * VAD_FRAME_SAMPLES;
const CAPACITY: usize = 4 * SHORT + OFFSET;

#[derive(Default)]
struct Phase {
    prior_short: Option<f32>,
    // The older entry ends exactly where the current double window begins.
    prior_doubles: [Option<f32>; 2],
}

pub(crate) struct SpeakerWindows {
    pcm: Vec<f32>,
    phases: [Phase; 2],
}

impl SpeakerWindows {
    pub(crate) fn new(encoder: SpeakerEncoder) -> Self {
        let capacity = if encoder == SpeakerEncoder::WeSpeakerResNet34LmV1 {
            CAPACITY
        } else {
            SHORT
        };
        Self {
            pcm: Vec::with_capacity(capacity),
            phases: Default::default(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.pcm.clear();
        self.phases = Default::default();
    }

    pub(crate) fn candidate(
        &mut self,
        frame: &[f32],
        gate: &SpeakerGate,
        embed: impl FnMut(&[f32]) -> EonResult<SpeakerEmbedding>,
        rejected: &mut u64,
    ) -> EonResult<Option<SpeakerEmbedding>> {
        // Silero checked the frame geometry before admission.
        debug_assert_eq!(frame.len(), VAD_FRAME_SAMPLES);
        self.pcm.extend_from_slice(frame);
        let n = self.pcm.len();
        let single = gate.encoder() != SpeakerEncoder::WeSpeakerResNet34LmV1;
        if n < SHORT {
            return Ok(None);
        }
        let phase = match n % SHORT {
            0 => 0,
            OFFSET if !single => 1,
            _ => return Ok(None),
        };
        let result = self.probe(phase, gate, embed, rejected);
        if single || !matches!(result, Ok(None)) {
            self.clear();
        } else if n == CAPACITY {
            // Both phases have now scored their longest eligible suffix.
            self.pcm.drain(..SHORT);
        }
        result
    }

    fn probe(
        &mut self,
        phase: usize,
        gate: &SpeakerGate,
        mut embed: impl FnMut(&[f32]) -> EonResult<SpeakerEmbedding>,
        rejected: &mut u64,
    ) -> EonResult<Option<SpeakerEmbedding>> {
        let pcm = &self.pcm;
        let mut score = |length| -> EonResult<_> {
            let candidate = embed(&pcm[pcm.len() - length..])?;
            let cosine = gate.score(&candidate).ok_or_else(|| {
                EonError::ValidationError("enrollment and candidate speaker encoders differ".into())
            })?;
            Ok((candidate, cosine))
        };
        // Count only embeddings actually compared to the acceptance threshold.
        let mut passes = |cosine: f32| {
            if cosine >= gate.threshold {
                true
            } else {
                *rejected = rejected.saturating_add(1);
                false
            }
        };
        let (short, recent) = score(SHORT)?;
        if passes(recent) {
            return Ok(Some(short));
        }
        if gate.encoder() != SpeakerEncoder::WeSpeakerResNet34LmV1 {
            return Ok(None);
        }
        let phase = &mut self.phases[phase];
        let Some(prior) = phase.prior_short.replace(recent) else {
            return Ok(None);
        };
        let (double, recent_double) = score(2 * SHORT)?;
        let [older_double, last_double] = phase.prior_doubles;
        phase.prior_doubles = [last_double, Some(recent_double)];

        // At each scale compare *disjoint*, equal-duration halves. A pooled
        // match isn't eligible if its trailing half lost identity evidence.
        if recent < prior {
            return Ok(None);
        }
        if passes(recent_double) {
            return Ok(Some(double));
        }
        if older_double.is_some_and(|older| recent_double >= older) {
            let (long, cosine) = score(4 * SHORT)?;
            if passes(cosine) {
                return Ok(Some(long));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WE: SpeakerEncoder = SpeakerEncoder::WeSpeakerResNet34LmV1;
    const F: usize = VAD_FRAME_SAMPLES;

    fn vector(encoder: SpeakerEncoder, cosine: f32) -> SpeakerEmbedding {
        let mut v = vec![0.; encoder.dimension()];
        v[0] = cosine;
        v[1] = (1. - cosine * cosine).sqrt();
        SpeakerEmbedding::from_values(encoder, &v).unwrap()
    }

    #[test]
    fn original_encoder_cadence_is_preserved_for_camplusplus() {
        let encoder = SpeakerEncoder::CamPlusPlusZhEnAdvancedV1;
        let mut windows = SpeakerWindows::new(encoder);
        let gate = SpeakerGate::enroll(&vector(encoder, 1.), 0.35).unwrap();
        let mut rejected = 0;
        for i in 1..=3 * SHORT / F {
            let candidate = windows
                .candidate(
                    &[0.; F],
                    &gate,
                    |pcm| {
                        assert_eq!(pcm.len(), SHORT);
                        assert_eq!(i % (SHORT / F), 0);
                        Ok(vector(encoder, 0.))
                    },
                    &mut rejected,
                )
                .unwrap();
            assert!(candidate.is_none());
        }
        assert_eq!(rejected, 3);
    }

    #[test]
    fn a_high_pooled_score_cannot_outvote_a_declining_trailing_half() {
        let gate = SpeakerGate::enroll(&vector(WE, 1.), 0.35).unwrap();
        // Three cases with identical 4-window cosine: declining evidence at
        // the short scale, declining at the double scale, and corroboration.
        for (short_falls, double_falls) in [(true, false), (false, true), (false, false)] {
            let mut windows = SpeakerWindows::new(WE);
            let mut rejected = 0;
            for i in 1..=4 * SHORT / F {
                let candidate = windows
                    .candidate(
                        &[i as f32; F],
                        &gate,
                        |pcm| {
                            assert_eq!(pcm[0], (i - pcm.len() / F + 1) as f32);
                            assert_eq!(pcm.last(), Some(&(i as f32)));
                            let recent = i * F == 4 * SHORT;
                            let cosine = match pcm.len() {
                                SHORT if recent && short_falls => -0.1,
                                len if len == 2 * SHORT && recent && double_falls => -0.1,
                                len if len == 4 * SHORT => 0.8,
                                _ => 0.,
                            };
                            Ok(vector(WE, cosine))
                        },
                        &mut rejected,
                    )
                    .unwrap();
                assert_eq!(
                    candidate.is_some(),
                    i * F == 4 * SHORT && !short_falls && !double_falls
                );
            }
        }
    }

    #[test]
    fn bounded_context_and_errors_never_splice_runs_of_speech() {
        let mut windows = SpeakerWindows::new(WE);
        let gate = SpeakerGate::enroll(&vector(WE, 1.), 0.35).unwrap();
        let mut rejected = 0;
        let mut count = [0; 3];
        for i in 1..=12 * SHORT / F {
            assert!(
                windows
                    .candidate(
                        &[i as f32; F],
                        &gate,
                        |pcm| {
                            let scale = [SHORT, 2 * SHORT, 4 * SHORT]
                                .iter()
                                .position(|&len| len == pcm.len())
                                .unwrap();
                            count[scale] += 1;
                            assert_eq!(pcm[0], (i - pcm.len() / F + 1) as f32);
                            assert_eq!(pcm.last(), Some(&(i as f32)));
                            Ok(vector(WE, 0.))
                        },
                        &mut rejected
                    )
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(count, [23, 21, 17]); // Both phases keep scoring across trims.
        assert_eq!(rejected, count.iter().sum::<u64>());
        assert_eq!(windows.pcm.capacity(), CAPACITY);
        for i in 1..=OFFSET / F {
            let result = windows.candidate(
                &[0.; F],
                &gate,
                |_| Err(EonError::ValidationError("probe".into())),
                &mut rejected,
            );
            assert_eq!(result.is_err(), i == OFFSET / F);
        }
        for _ in 0..SHORT / F - 1 {
            assert!(
                windows
                    .candidate(
                        &[1.; F],
                        &gate,
                        |_| panic!("pre-error audio retained"),
                        &mut rejected
                    )
                    .unwrap()
                    .is_none()
            );
        }
        // An explicit playback/VAD gap must discard even these fresh frames.
        windows.clear();
        for _ in 0..SHORT / F - 1 {
            assert!(
                windows
                    .candidate(
                        &[2.; F],
                        &gate,
                        |_| panic!("pre-gap audio retained"),
                        &mut rejected
                    )
                    .unwrap()
                    .is_none()
            );
        }
        assert!(
            windows
                .candidate(
                    &[2.; F],
                    &gate,
                    |pcm| {
                        assert!(pcm.iter().all(|&x| x == 2.));
                        Ok(vector(WE, 1.))
                    },
                    &mut rejected
                )
                .unwrap()
                .is_some()
        );
    }
}
