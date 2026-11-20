//! EON Cyber Engine: program analysis, taint slicing, and armor checks.
#[cfg(feature = "solver")]
pub mod branches;
pub mod cpg;
pub mod defense;
pub mod function_review;
pub mod interproc;
pub mod remediation;
pub mod sbom;
#[cfg(feature = "solver")]
pub mod solver;
pub mod taint;

pub use eon_primitives::*;
