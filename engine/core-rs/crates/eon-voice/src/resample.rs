//! Sample-rate conversion for the audio pipeline (docs/implementation/04, resampling stage).
//!
//! Device audio arrives at the hardware rate (often 48 kHz). The VAD and ASR models need 16 kHz. This module converts
//! a mono clip with a windowed-sinc resampler (rubato). The resampler's startup delay is handled by `process_all`, so
//! no leading silence is left in the output.

use eon_primitives::{EonError, EonResult};
use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType, WindowFunction};

const SINC_LEN: usize = 128;
const OVERSAMPLING: usize = 256;
const CUTOFF: f32 = 0.95;
const CHUNK_FRAMES: usize = 1024;
const MAX_RATIO_RELATIVE: f64 = 1.1;

fn invalid(reason: impl Into<String>) -> EonError {
    EonError::ValidationError(reason.into())
}

/// Converts mono samples from `from_hz` to `to_hz`. Equal rates return a copy.
pub fn resample_mono(input: &[f32], from_hz: u32, to_hz: u32) -> EonResult<Vec<f32>> {
    if from_hz == 0 || to_hz == 0 {
        return Err(invalid("sample rates must be non-zero"));
    }
    if from_hz == to_hz || input.is_empty() {
        return Ok(input.to_vec());
    }
    let parameters = SincInterpolationParameters {
        sinc_len: SINC_LEN,
        f_cutoff: Some(CUTOFF),
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: OVERSAMPLING,
        window: WindowFunction::BlackmanHarris2,
    };
    let ratio = f64::from(to_hz) / f64::from(from_hz);
    let mut resampler = Async::<f32>::new_sinc(
        ratio,
        MAX_RATIO_RELATIVE,
        &parameters,
        CHUNK_FRAMES,
        1,
        FixedAsync::Input,
    )
    .map_err(|e| invalid(format!("resampler setup failed: {e}")))?;

    let channels = vec![input.to_vec()];
    let adapter = SequentialSliceOfVecs::new(&channels, 1, input.len())
        .map_err(|e| invalid(format!("resampler input: {e}")))?;
    let output = resampler
        .process_all(&adapter, input.len(), None)
        .map_err(|e| invalid(format!("resampling failed: {e}")))?;
    Ok(output.take_data())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, rate: u32, seconds: f32) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    /// Counts zero crossings in the central part of the signal, which gives the dominant frequency without the edges.
    fn estimated_hz(samples: &[f32], rate: u32) -> f32 {
        let core = &samples[samples.len() / 4..samples.len() * 3 / 4];
        let crossings = core.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        let seconds = core.len() as f32 / rate as f32;
        crossings as f32 / (2.0 * seconds)
    }

    #[test]
    fn a_48k_tone_becomes_a_16k_tone_at_the_same_frequency() {
        let input = tone(440.0, 48_000, 1.0);
        let out = resample_mono(&input, 48_000, 16_000).unwrap();
        let expected_len = 16_000f32;
        assert!((out.len() as f32 - expected_len).abs() < 16.0, "length {}", out.len());
        let hz = estimated_hz(&out, 16_000);
        assert!((hz - 440.0).abs() < 4.0, "estimated {hz} Hz");
    }

    #[test]
    fn equal_rates_are_a_copy_and_bad_rates_are_refused() {
        let input = tone(100.0, 16_000, 0.1);
        assert_eq!(resample_mono(&input, 16_000, 16_000).unwrap(), input);
        assert!(resample_mono(&input, 0, 16_000).is_err());
        assert!(resample_mono(&input, 16_000, 0).is_err());
    }
}
