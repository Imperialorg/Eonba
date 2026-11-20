use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eon_primitives::{EonError, EonResult};
use eon_voice::asr::ZipformerAsr;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use super::{MAX_PCM_BYTES, valid_text};

#[derive(Deserialize)]
struct Asset {
    id: String,
    dest: PathBuf,
    sha256: Option<String>,
    size_bytes: Option<u64>,
}

#[derive(Deserialize)]
struct Lock {
    models: Vec<Asset>,
}

pub(super) fn unavailable() -> EonError {
    EonError::CapabilityNotFound {
        capability_id: "pinned Zipformer ASR".into(),
    }
}

/// A preloaded session, shared by at most one inference at a time. No downloads,
/// model selection, runtime paths or session settings are accepted over the wire.
pub struct PinnedZipformer(Arc<Mutex<ZipformerAsr>>);

impl PinnedZipformer {
    /// Blocking startup, not for the audio thread. Use a trusted local model
    /// installation that does not change while models are loaded. The runtime
    /// is the one linked by eon-voice; session initialization failures are errors.
    pub fn load(model_dir: &Path) -> EonResult<Self> {
        let lock: Lock = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../scripts/models.lock.json"
        )))
        .map_err(|_| unavailable())?;
        for component in ["encoder-int8", "decoder", "joiner-int8", "tokens"] {
            let id = format!("zipformer-en-2023-06-26-{component}");
            let asset = lock.models.iter().find(|m| m.id == id).ok_or_else(unavailable)?;
            let file = File::open(model_dir.join(asset.dest.file_name().ok_or_else(unavailable)?))
                .map_err(|_| unavailable())?;
            let size = asset.size_bytes.ok_or_else(unavailable)?;
            let mut digest = Sha256::new();
            if io::copy(&mut file.take(size + 1), &mut digest).map_err(|_| unavailable())? != size
                || format!("{:x}", digest.finalize()) != *asset.sha256.as_ref().ok_or_else(unavailable)?
            {
                return Err(unavailable());
            }
        }
        let model = ZipformerAsr::load(model_dir).map_err(|_| unavailable())?;
        Ok(Self(Arc::new(Mutex::new(model))))
    }

    pub(super) async fn infer(&self, pcm: Vec<u8>) -> EonResult<String> {
        if pcm.is_empty() || pcm.len() > MAX_PCM_BYTES || !pcm.len().is_multiple_of(2) {
            return Err(EonError::ValidationError("invalid PCM16 utterance".into()));
        }
        // Even if the caller times out, the guard stays with the bounded CPU job.
        // Don't queue more work behind an in-flight call or block the QUIC reactor.
        let mut model = self.0.clone().try_lock_owned().map_err(|_| unavailable())?;
        tokio::task::spawn_blocking(move || {
            let samples: Vec<_> = pcm
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&v| f32::from(i16::from_le_bytes(v)) / 32768.0)
                .collect();
            let text = model.transcribe(&samples).map_err(|_| unavailable())?;
            valid_text(&text)?;
            Ok(text)
        })
        .await
        .map_err(|_| unavailable())?
    }
}
