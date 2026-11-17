//! Fixed sample delay for aligning the playback reference with the echo (docs/implementation/04, echo path).
//!
//! The echo canceller needs the render signal at the same time offset as the microphone sees its echo. The playback
//! path already has a known latency, so the reference is held back by that amount before it reaches the canceller.

pub struct DelayLine {
    buffer: Vec<f32>,
    position: usize,
}

impl DelayLine {
    /// A delay of `samples` samples. Zero is a pass-through.
    pub fn new(samples: usize) -> Self {
        Self {
            buffer: vec![0.0; samples],
            position: 0,
        }
    }

    /// Returns the sample from `samples` calls ago, or zero before the line has filled.
    pub fn process(&mut self, sample: f32) -> f32 {
        if self.buffer.is_empty() {
            return sample;
        }
        let delayed = self.buffer[self.position];
        self.buffer[self.position] = sample;
        self.position = (self.position + 1) % self.buffer.len();
        delayed
    }

    /// Delays a block in place.
    pub fn process_block(&mut self, block: &mut [f32]) {
        for sample in block.iter_mut() {
            *sample = self.process(*sample);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_the_input_shifted_by_the_delay() {
        let mut line = DelayLine::new(3);
        let out: Vec<f32> = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0].iter().map(|x| line.process(*x)).collect();
        assert_eq!(out, vec![0.0, 0.0, 0.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn zero_delay_is_a_pass_through() {
        let mut line = DelayLine::new(0);
        assert_eq!(line.process(0.5), 0.5);
        let mut block = [0.1, 0.2];
        line.process_block(&mut block);
        assert_eq!(block, [0.1, 0.2]);
    }
}
