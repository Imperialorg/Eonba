//! Peak-limiting ASR gain controller (docs/implementation/04, section 3.2).
//!
//! Quiet phone speech is boosted, loud near-field speech is not allowed to clip, and the gain is applied
//! only to the float copy sent to ASR. Attack is instant so a sudden peak never clips. Release is smoothed so
//! gain does not pump between words.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PcmGainController {
    pub requested_gain: f32,
    pub target_peak: f32,
    pub release_alpha: f32,
    current_gain: f32,
}

impl PcmGainController {
    pub fn new(requested_gain: f32, target_peak: f32, release_alpha: f32) -> Self {
        Self {
            requested_gain,
            target_peak,
            release_alpha,
            current_gain: 1.0,
        }
    }

    /// The spec's defaults: boost up to 8x, cap peaks at 0.95, release at 0.08 per block.
    pub fn asr_default() -> Self {
        Self::new(8.0, 0.95, 0.08)
    }

    pub fn current_gain(&self) -> f32 {
        self.current_gain
    }

    /// Scales one block in place and returns the gain used. Output is always within [-target_peak, target_peak].
    pub fn process(&mut self, block: &mut [f32]) -> f32 {
        let peak = block.iter().fold(0.0f32, |p, &x| p.max(x.abs()));
        let headroom_gain = if peak > 0.0 { self.target_peak / peak } else { f32::INFINITY };
        let desired = self.requested_gain.min(headroom_gain);
        self.current_gain = if desired < self.current_gain {
            desired
        } else {
            self.current_gain + self.release_alpha * (desired - self.current_gain)
        };
        for sample in block.iter_mut() {
            *sample = (*sample * self.current_gain).clamp(-self.target_peak, self.target_peak);
        }
        self.current_gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_speech_is_boosted_toward_the_requested_gain() {
        let mut gain = PcmGainController::asr_default();
        let mut block = vec![0.03f32; 160];
        for _ in 0..200 {
            block.iter_mut().for_each(|x| *x = 0.03);
            gain.process(&mut block);
        }
        assert!(gain.current_gain() > 7.9, "gain converges to 8x, got {}", gain.current_gain());
    }

    #[test]
    fn loud_speech_is_never_clipped() {
        let mut gain = PcmGainController::asr_default();
        for amplitude in [0.2f32, 0.5, 0.9, 1.0] {
            let mut block: Vec<f32> = (0..256).map(|i| if i % 2 == 0 { amplitude } else { -amplitude }).collect();
            gain.process(&mut block);
            assert!(block.iter().all(|x| x.abs() <= 0.95 + 1e-6), "clipped at amplitude {amplitude}");
        }
    }

    #[test]
    fn attack_is_instant_so_a_sudden_peak_is_capped_in_the_same_block() {
        let mut gain = PcmGainController::asr_default();
        let mut quiet = vec![0.01f32; 64];
        for _ in 0..300 {
            gain.process(&mut quiet);
        }
        let mut sudden = vec![0.9f32; 64];
        gain.process(&mut sudden);
        assert!(sudden.iter().all(|x| x.abs() <= 0.95 + 1e-6));
    }

    #[test]
    fn release_is_smoothed_not_instant() {
        let mut gain = PcmGainController::asr_default();
        let mut loud = vec![0.5f32; 32];
        gain.process(&mut loud);
        let after_loud = gain.current_gain();
        let mut silent = vec![0.0f32; 32];
        gain.process(&mut silent);
        let one_step = gain.current_gain();
        assert!(one_step > after_loud && one_step < 8.0, "gain should rise gradually");
    }

    #[test]
    fn silence_does_not_divide_by_zero() {
        let mut gain = PcmGainController::asr_default();
        let mut silent = vec![0.0f32; 32];
        let used = gain.process(&mut silent);
        assert!(used.is_finite());
        assert!(silent.iter().all(|x| *x == 0.0));
    }
}
