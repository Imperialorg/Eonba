//! Frame geometry shared by the audio loop (docs/implementation/04, section 2).
//!
//! Each pipeline stage works on a fixed block size. Mixing them up misaligns AEC reference and capture by whole
//! frames, so the conversions live here, tested, rather than as bare literals at call sites.

pub const SAMPLE_RATE_HZ: u32 = 16_000;
/// Silero VAD v5 requires 512 samples (32 ms) at 16 kHz. The 480-sample value in older notes is wrong for v5.
pub const VAD_FRAME_SAMPLES: usize = 512;
/// AEC3 delay-aligned block: 10 ms at 16 kHz.
pub const AEC_BLOCK_SAMPLES: usize = 160;
/// Zipformer streaming chunk (`chunk=16`, 320 ms).
pub const ZIPFORMER_CHUNK_MS: u32 = 320;

pub const fn samples_for_ms(ms: u32, sample_rate_hz: u32) -> usize {
    (ms as u64 * sample_rate_hz as u64 / 1000) as usize
}

pub const fn ms_for_samples(samples: usize, sample_rate_hz: u32) -> u32 {
    (samples as u64 * 1000 / sample_rate_hz as u64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vad_frame_is_32_ms() {
        assert_eq!(ms_for_samples(VAD_FRAME_SAMPLES, SAMPLE_RATE_HZ), 32);
    }

    #[test]
    fn aec_block_is_10_ms() {
        assert_eq!(ms_for_samples(AEC_BLOCK_SAMPLES, SAMPLE_RATE_HZ), 10);
    }

    #[test]
    fn zipformer_chunk_is_320_ms_worth_of_samples() {
        assert_eq!(samples_for_ms(ZIPFORMER_CHUNK_MS, SAMPLE_RATE_HZ), 5120);
    }

    #[test]
    fn conversions_round_trip_on_whole_frames() {
        for ms in [10u32, 32, 320] {
            assert_eq!(ms_for_samples(samples_for_ms(ms, SAMPLE_RATE_HZ), SAMPLE_RATE_HZ), ms);
        }
    }
}
