//! EON Capability Fabric: extensible tool contracts, sandboxing, and providers.
pub mod ifc;
pub mod mcp_pinning;
pub mod melon;
pub mod quarantine;
pub mod registry;
pub mod sandbox;
pub mod schema;
pub mod wasm_tool;

pub use eon_primitives::*;
