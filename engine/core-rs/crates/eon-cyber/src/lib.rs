//! EON Cyber Engine: program analysis, taint slicing, and armor checks.
#[cfg(feature = "solver")]
pub mod branches;
pub mod cpg;
pub mod interproc;
pub mod sbom;
#[cfg(feature = "solver")]
pub mod solver;
pub mod taint;

pub use eon_primitives::*;
