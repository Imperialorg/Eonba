//! Kaldi-compatible 80-bin log-mel features shared by the ASR and speaker models (docs/implementation/04).

use eon_primitives::{EonError, EonResult};
use kaldi_native_fbank::online::FeatureComputer;
use kaldi_native_fbank::{FbankComputer, FbankOptions, OnlineFeature};

pub const SAMPLE_RATE_HZ: f32 = 16_000.0;
pub const NUM_MEL_BINS: usize = 80;

/// Computes one 80-dimensional log-mel vector per 10 ms frame of 16 kHz mono audio. Dither is off and edges are not
/// snipped, so the output is deterministic and matches the reference front end.
pub fn kaldi_fbank(samples: &[f32]) -> EonResult<Vec<Vec<f32>>> {
    let mut features = online_fbank()?;
    features.accept_waveform(SAMPLE_RATE_HZ, samples);
    features.input_finished();
    Ok(features.features)
}

pub(crate) fn online_fbank() -> EonResult<OnlineFeature> {
    let mut opts = FbankOptions::default();
    opts.frame_opts.dither = 0.0;
    opts.frame_opts.snip_edges = false;
    opts.mel_opts.num_bins = NUM_MEL_BINS;
    opts.use_energy = false;
    let computer = FbankComputer::new(opts).map_err(|e| EonError::OperationFailed {
        operation: "features.fbank".into(),
        reason: e,
    })?;
    Ok(OnlineFeature::new(FeatureComputer::Fbank(computer)))
}
