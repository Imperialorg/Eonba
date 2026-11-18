//! Chunked bulk transfer with per-chunk BLAKE3 verification (docs/implementation/05, section 2.2, Class 3).
//!
//! The manifest lists the size and a BLAKE3 hash for each chunk. It must come from a trusted source, such as an epoch
//! commit, because a manifest supplied by the peer that serves the bytes proves nothing. Each chunk is verified as it
//! arrives, so a corrupt or substituted chunk is refused and the transfer can resume from the missing indices.

use std::time::Instant;

use eon_primitives::{EonError, EonResult};
use iroh::EndpointAddr;

use crate::qos::{BulkPacer, TrafficClass};
use crate::transport::PeerNode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    size: usize,
    chunk_size: usize,
    chunk_hashes: Vec<[u8; 32]>,
}

fn blob_err(reason: impl Into<String>) -> EonError {
    EonError::OperationFailed {
        operation: "mesh.blob".into(),
        reason: reason.into(),
    }
}

impl Manifest {
    pub fn for_bytes(data: &[u8], chunk_size: usize) -> EonResult<Self> {
        if chunk_size == 0 {
            return Err(blob_err("chunk size must be non-zero"));
        }
        let chunk_hashes = data.chunks(chunk_size).map(|c| *blake3::hash(c).as_bytes()).collect();
        Ok(Self {
            size: data.len(),
            chunk_size,
            chunk_hashes,
        })
    }

    pub fn chunk_count(&self) -> usize {
        self.chunk_hashes.len()
    }

    /// Length of chunk `index`. Every chunk is `chunk_size` bytes except the last, which takes the remainder.
    pub fn chunk_len(&self, index: usize) -> Option<usize> {
        if index >= self.chunk_count() {
            return None;
        }
        Some(self.chunk_size.min(self.size - index * self.chunk_size))
    }

    pub fn verify_chunk(&self, index: usize, bytes: &[u8]) -> EonResult<()> {
        let expected = self
            .chunk_len(index)
            .ok_or_else(|| blob_err(format!("chunk {index} is outside the manifest")))?;
        if bytes.len() != expected {
            return Err(blob_err(format!(
                "chunk {index} is {} bytes, expected {expected}",
                bytes.len()
            )));
        }
        if blake3::hash(bytes).as_bytes() != &self.chunk_hashes[index] {
            return Err(blob_err(format!("chunk {index} failed BLAKE3 verification")));
        }
        Ok(())
    }
}

/// Collects verified chunks in any order. Nothing is returned until every chunk is present.
pub struct Assembler {
    manifest: Manifest,
    chunks: Vec<Option<Vec<u8>>>,
}

impl Assembler {
    pub fn new(manifest: Manifest) -> Self {
        let count = manifest.chunk_count();
        Self {
            manifest,
            chunks: vec![None; count],
        }
    }

    pub fn accept(&mut self, index: usize, bytes: &[u8]) -> EonResult<()> {
        self.manifest.verify_chunk(index, bytes)?;
        self.chunks[index] = Some(bytes.to_vec());
        Ok(())
    }

    pub fn missing(&self) -> Vec<usize> {
        self.chunks
            .iter()
            .enumerate()
            .filter_map(|(index, chunk)| chunk.is_none().then_some(index))
            .collect()
    }

    pub fn finish(self) -> EonResult<Vec<u8>> {
        let missing = self.missing();
        if !missing.is_empty() {
            return Err(blob_err(format!("{} chunks still missing", missing.len())));
        }
        Ok(self.chunks.into_iter().flatten().flatten().collect())
    }
}

/// Fetches every chunk of `manifest` from `server`, one Bulk-class stream per chunk. Each chunk waits for `pacer` to admit
/// it, and is verified before it is accepted.
pub async fn fetch(
    client: &PeerNode,
    server: EndpointAddr,
    manifest: &Manifest,
    pacer: &mut BulkPacer,
) -> EonResult<Vec<u8>> {
    let started = Instant::now();
    let mut assembler = Assembler::new(manifest.clone());
    for index in 0..manifest.chunk_count() {
        let len = manifest.chunk_len(index).expect("index is within the manifest");
        let may_start_at = pacer.admit(started.elapsed(), len);
        tokio::time::sleep(may_start_at.saturating_sub(started.elapsed())).await;
        let chunk = client
            .request_with_class(server.clone(), TrafficClass::Bulk, &(index as u32).to_le_bytes())
            .await?;
        assembler.accept(index, &chunk)?;
    }
    assembler.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        (0..1000u32).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn chunks_assemble_in_any_order_into_the_original_bytes() {
        let data = sample();
        let manifest = Manifest::for_bytes(&data, 300).unwrap();
        assert_eq!(manifest.chunk_count(), 4);
        assert_eq!(manifest.chunk_len(3), Some(100), "the last chunk takes the remainder");

        let mut assembler = Assembler::new(manifest);
        for index in [2usize, 0, 3, 1] {
            let chunk = &data[index * 300..(index * 300 + 300).min(data.len())];
            assembler.accept(index, chunk).unwrap();
        }
        assert!(assembler.missing().is_empty());
        assert_eq!(assembler.finish().unwrap(), data);
    }

    #[test]
    fn a_substituted_chunk_is_refused_and_leaves_the_slot_missing() {
        let data = sample();
        let mut assembler = Assembler::new(Manifest::for_bytes(&data, 300).unwrap());
        let mut tampered = data[300..600].to_vec();
        tampered[10] ^= 1;
        let err = assembler.accept(1, &tampered).unwrap_err().to_string();
        assert!(err.contains("chunk 1 failed BLAKE3 verification"), "{err}");
        assert_eq!(assembler.missing(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn a_wrong_length_chunk_is_refused() {
        let data = sample();
        let mut assembler = Assembler::new(Manifest::for_bytes(&data, 300).unwrap());
        assert!(assembler.accept(0, &data[..299]).is_err());
    }

    #[test]
    fn finish_refuses_an_incomplete_transfer() {
        let data = sample();
        let mut assembler = Assembler::new(Manifest::for_bytes(&data, 300).unwrap());
        assembler.accept(0, &data[..300]).unwrap();
        assert!(assembler.finish().is_err());
    }

    #[test]
    fn zero_chunk_size_is_refused() {
        assert!(Manifest::for_bytes(&sample(), 0).is_err());
    }
}
