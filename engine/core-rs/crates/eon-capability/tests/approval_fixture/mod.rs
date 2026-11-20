use std::fs;
use std::ops::{Deref, DerefMut};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use eon_capability::{
    EonResult,
    registry::{AllowedTool, CapabilityRegistry, DurableApprovals},
};

pub struct Fixture {
    pub root: PathBuf,
    registry: CapabilityRegistry,
}

impl Deref for Fixture {
    type Target = CapabilityRegistry;
    fn deref(&self) -> &Self::Target {
        &self.registry
    }
}

impl DerefMut for Fixture {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.registry
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

pub fn prepared<const N: usize>(entries: [AllowedTool; N]) -> EonResult<Fixture> {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("approved-{:016x}", getrandom::u64().unwrap()));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let mut fixture = Fixture {
        root,
        registry: CapabilityRegistry::new(),
    };
    let store = DurableApprovals::open(&fixture.root)?;
    for entry in &entries {
        store.approve(entry)?;
    }
    fixture.registry = CapabilityRegistry::with_allowlist(entries, store)?;
    Ok(fixture)
}
