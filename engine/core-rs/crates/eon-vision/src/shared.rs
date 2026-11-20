//! Linux frame storage for an explicitly selected local reader. Only the JPEG is shared:
//! it contains no ring metadata, earlier frames, or authority to observe subsequent frames.

use std::fs::File;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::FileExt;

use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};

use crate::keyframes::{KeyframeHistory, Luma};
use crate::{EonError, EonResult};

/// The descriptor can be passed explicitly or mapped read-only. Use positional reads or
/// mmap: duplicated descriptors share a seek position. Dropping the ring's reference does
/// not revoke an already delivered descriptor or erase a recipient's copy.
#[derive(Debug)]
pub struct SharedJpeg(File);

impl AsFd for SharedJpeg {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

fn store(data: &[u8]) -> EonResult<SharedJpeg> {
    let failed = |e: std::io::Error| EonError::OperationFailed {
        operation: "store shared keyframe".into(),
        reason: e.to_string(),
    };
    let fd = memfd_create(
        c"eon-keyframe",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )
    .map_err(|e| failed(e.into()))?;
    let file = File::from(fd);
    file.write_all_at(data, 0).map_err(failed)?;
    fcntl_add_seals(
        &file,
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    )
    .map_err(|e| failed(e.into()))?;
    Ok(SharedJpeg(file))
}

/// A rolling history backed by one anonymous, sealed memory file per sampled frame.
/// The producer keeps at most 30 seconds (150 frames) and applies the same byte limit as
/// the portable history. The caller supplies already-authorized captures; nothing is sent.
pub type SharedVisualMemory = KeyframeHistory<SharedJpeg>;

impl SharedVisualMemory {
    pub fn observe(
        &mut self,
        now_ms: u64,
        dimensions: (u32, u32),
        thumbnail: &Luma,
        jpeg: &[u8],
    ) -> EonResult<Option<u64>> {
        self.observe_with(now_ms, dimensions, thumbnail, jpeg, store)
    }
}
