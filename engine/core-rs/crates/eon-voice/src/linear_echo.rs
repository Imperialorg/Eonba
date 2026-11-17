//! Normalised LMS echo canceller for barge-in detection (docs/implementation/04, echo path).
//!
//! The barge-in detector needs to hear the owner over the assistant's own echo. AEC3's suppressor removes near-end
//! speech while the render is active, so its output cannot be used to detect the owner (see the loopback test). This
//! canceller models only the linear echo from the render, so uncorrelated speech passes through unchanged. It is not a
//! substitute for AEC3 on the ASR path.

pub struct NlmsCanceller {
    weights: Vec<f32>,
    history: Vec<f32>,
    head: usize,
    power: f32,
    mu: f32,
}

const POWER_FLOOR: f32 = 1e-6;

impl NlmsCanceller {
    /// `taps` must cover the echo delay plus its tail. `mu` is the step size, in (0, 2).
    pub fn new(taps: usize, mu: f32) -> Self {
        Self {
            weights: vec![0.0; taps],
            history: vec![0.0; taps],
            head: 0,
            power: 0.0,
            mu,
        }
    }

    /// Adds one playback sample to the reference history.
    pub fn push_render(&mut self, sample: f32) {
        let taps = self.history.len();
        let oldest = self.history[self.head];
        self.power = (self.power - oldest * oldest).max(0.0) + sample * sample;
        self.history[self.head] = sample;
        self.head = (self.head + 1) % taps;
    }

    /// Returns the microphone sample minus the predicted echo, and adapts the weights.
    pub fn process(&mut self, mic: f32) -> f32 {
        let taps = self.weights.len();
        let mut predicted = 0.0f32;
        for k in 0..taps {
            let index = (self.head + taps - 1 - k) % taps;
            predicted += self.weights[k] * self.history[index];
        }
        let residual = mic - predicted;
        let gain = self.mu * residual / (self.power + POWER_FLOOR);
        for k in 0..taps {
            let index = (self.head + taps - 1 - k) % taps;
            self.weights[k] += gain * self.history[index];
        }
        residual
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_is_removed_and_uncorrelated_speech_survives() {
        let rate = 16_000.0f32;
        let delay = 720;
        let total = 48_000;
        let owner_start = 32_000;
        let mut canceller = NlmsCanceller::new(1024, 0.2);
        let render: Vec<f32> = (0..total)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate).sin() * 0.4)
            .collect();
        let mut lcg = 12345u64;
        let owner: Vec<f32> = (0..total)
            .map(|i| {
                lcg = lcg.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                if i < owner_start {
                    0.0
                } else {
                    ((lcg >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.2
                }
            })
            .collect();

        let mut echo_only_residual = 0.0f32;
        let mut echo_only_count = 0usize;
        let mut owner_error = 0.0f32;
        let mut owner_energy = 0.0f32;
        for i in 0..total {
            canceller.push_render(render[i]);
            let echo = if i >= delay { render[i - delay] * 0.5 } else { 0.0 };
            let residual = canceller.process(echo + owner[i]);
            if (24_000..owner_start).contains(&i) {
                echo_only_residual += residual * residual;
                echo_only_count += 1;
            }
            if i >= owner_start + 8_000 {
                owner_error += (residual - owner[i]).powi(2);
                owner_energy += owner[i] * owner[i];
            }
        }
        let echo_only_power = echo_only_residual / echo_only_count as f32;
        assert!(echo_only_power < 1e-4, "echo residual power {echo_only_power} after convergence");
        assert!(
            owner_error < owner_energy * 0.2,
            "owner speech distorted: error {owner_error} against energy {owner_energy}"
        );
    }
}
