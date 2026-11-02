//! Test support shared by every crate: the scripted fake provider, the fake tool, synthetic
//! evaluation sets, and the fake MCP server (the `squint-testkit-mcp` binary, located by `mcp_server::path`). A normal dependency
//! of the `squint` binary only behind its `testkit` feature.

pub mod images;
pub mod mcp_client;
pub mod mcp_server;
pub mod provider;
pub mod sets;
pub mod tool;

pub use provider::{FAKE_PRICES, ScriptTurn, ScriptedPricing, ScriptedProvider};
pub use sets::synthetic_set;
pub use tool::ScriptedTool;

/// Errors this crate produces.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("script: {0}")]
    Script(String),
}

pub type Result<T> = std::result::Result<T, Error>;
