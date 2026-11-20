use std::collections::{BTreeMap, btree_map::Entry};
use std::fs::File;
use std::io::Write;
use std::ops::Range;
use std::os::unix::fs::FileExt;

use eon_primitives::EonResult;
use memmap2::{Mmap, MmapMut, MmapOptions};
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};

use super::workspace::failed;
use super::{
    CompiledObjective, MAX_READ_BYTES, ReadRequest, ReadWorkspace, ShadowRead, StagedPreview,
    invalid,
};

/// Private CoW view of exactly the authorized extents, in task order. Not a
/// checkout: no whole-file copies, source handles, path lookup or merge-back.
pub struct ReadSnapshot {
    plan: CompiledObjective,
    view: Mmap,
    extents: Vec<Extent>,
}

struct Extent {
    file_len: usize,
    packed: Range<usize>,
}

impl CompiledObjective {
    /// Capture the read set, not the workspace. Requires Linux openat2, procfs,
    /// sealable memfd and MAP_PRIVATE; unsupported operations fail closed.
    pub fn snapshot(self, workspace: &ReadWorkspace) -> EonResult<ReadSnapshot> {
        let (base, extents) = capture(workspace, self.reads.iter())?;
        base.branch(self, extents)
    }
}

pub(super) fn alternatives(
    workspace: &ReadWorkspace,
    left: CompiledObjective,
    right: CompiledObjective,
) -> EonResult<[StagedPreview; 2]> {
    let (base, mut extents) = capture(workspace, left.reads.iter().chain(&right.reads))?;
    let other = extents.split_off(left.reads.len());
    Ok([
        base.branch(left, extents)?.preview(),
        base.branch(right, other)?.preview(),
    ])
}

fn capture<'a>(
    workspace: &ReadWorkspace,
    reads: impl Iterator<Item = &'a ReadRequest> + Clone,
) -> EonResult<(Sealed, Vec<Extent>)> {
    let mut sources = BTreeMap::new();
    for request in reads.clone() {
        if let Entry::Vacant(slot) = sources.entry(request.path.as_str()) {
            slot.insert(workspace.source(&request.path)?);
        }
    }

    let mut image = new_image()?;
    let mut end = 0;
    let mut extents = Vec::new();
    let mut scratch = [0; MAX_READ_BYTES];
    for request in reads {
        let source = &sources[request.path.as_str()];
        let remaining = source
            .len()
            .checked_sub(request.offset)
            .ok_or_else(|| invalid("offset beyond end of workspace file"))?;
        let count = remaining.min(request.length as u64) as usize;
        source
            .file
            .read_exact_at(&mut scratch[..count], request.offset)
            .map_err(failed)?;
        image.write_all(&scratch[..count]).map_err(failed)?;
        extents.push(Extent {
            file_len: usize::try_from(source.len())
                .map_err(|_| invalid("unrepresentable file length"))?,
            packed: end..end + count,
        });
        end += count;
    }
    for (path, source) in sources {
        workspace.revalidate(path, &source)?;
    }
    if end == 0 {
        // mmap requires a nonzero backing length; no extent includes this byte.
        image.set_len(1).map_err(failed)?;
    }
    Ok((seal(image)?, extents))
}

impl ReadSnapshot {
    /// Execute only the compiled read plan. The anonymous mapping is destroyed
    /// before presentation, even if the user never responds. Only the bounded
    /// results are held behind the existing one-shot attention/consent gate.
    pub fn preview(self) -> StagedPreview {
        let reads = self
            .plan
            .reads
            .into_iter()
            .zip(self.extents)
            .map(|(request, extent)| ShadowRead {
                request,
                file_len: extent.file_len,
                bytes: self.view[extent.packed].to_vec(),
            })
            .collect();
        StagedPreview::new(self.plan.workflow, reads)
    }
}

fn new_image() -> EonResult<File> {
    memfd_create(
        c"eon-read-snapshot",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )
    .map(File::from)
    .map_err(failed)
}

struct Sealed(File);

fn seal(image: File) -> EonResult<Sealed> {
    fcntl_add_seals(
        &image,
        SealFlags::WRITE | SealFlags::SHRINK | SealFlags::GROW | SealFlags::SEAL,
    )
    .map_err(failed)?;
    Ok(Sealed(image))
}

impl Sealed {
    fn branch(&self, plan: CompiledObjective, extents: Vec<Extent>) -> EonResult<ReadSnapshot> {
        Ok(ReadSnapshot {
            plan,
            view: self.fork()?.make_read_only().map_err(failed)?,
            extents,
        })
    }

    fn fork(&self) -> EonResult<MmapMut> {
        // SAFETY: this is a sealed regular memfd, never the source file. No fd
        // can write to or truncate its backing. Writes to MAP_PRIVATE diverge.
        unsafe { MmapOptions::new().map_copy(&self.0) }.map_err(failed)
    }
}

#[cfg(test)]
mod compiler_tests {
    use super::*;

    #[test]
    fn private_pages_diverge_without_changing_the_sealed_base_or_another_view() {
        let mut image = new_image().unwrap();
        image.write_all(b"base").unwrap();
        let sealed = seal(image).unwrap();
        let mut first = sealed.fork().unwrap();
        let second = sealed.fork().unwrap();
        first[0] = b'v';
        assert_eq!(&first[..], b"vase");
        assert_eq!(&second[..], b"base");
        let mut bytes = [0; 4];
        sealed.0.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"base");
        assert!(sealed.0.write_at(b"x", 0).is_err());
        assert!(sealed.0.set_len(0).is_err());
    }

    #[test]
    fn anonymous_backing_is_unmapped_before_the_consent_window() {
        use std::os::unix::fs::MetadataExt;
        let mut image = new_image().unwrap();
        image.write_all(b"test").unwrap();
        let inode = image.metadata().unwrap().ino().to_string();
        let mapped = || {
            std::fs::read_to_string("/proc/self/maps")
                .unwrap()
                .lines()
                .any(|line| {
                    line.contains("/memfd:eon-read-snapshot")
                        && line.split_ascii_whitespace().nth(4) == Some(&inode)
                })
        };
        let objective = serde_json::from_value(serde_json::json!({
            "id": eon_kernel::ObjectiveId::new(),
            "owner_id": eon_kernel::IdentityId::new(),
            "description": "read file A", "priority": "Normal",
            "created_at_ns": 10, "deadline_ns": null, "epoch": 7,
        }))
        .unwrap();
        let plan = super::super::compile(&objective).unwrap();
        let snapshot = seal(image)
            .unwrap()
            .branch(
                plan,
                vec![Extent {
                    file_len: 4,
                    packed: 0..4,
                }],
            )
            .unwrap();
        assert!(mapped());
        let _staged = snapshot.preview();
        assert!(!mapped());
    }
}
