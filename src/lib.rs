//! Local video inspection over MCP. `mcp` is the tool layer and the only module
//! that uses rmcp; `main.rs` serves it over stdio. Every other module is the media
//! engine: synchronous work that async callers reach through `service::Service`,
//! which runs it on bounded blocking workers.
pub mod control;
pub mod detail;
pub mod mcp;
#[cfg(feature = "native")]
pub mod media;
pub mod ocr;
pub mod orientation;
pub mod service;
pub mod source;
#[cfg(feature = "whisper")]
pub mod speech;
pub mod timestamp;
pub mod tracking;
pub mod types;
#[cfg(feature = "native")]
mod video;
#[cfg(feature = "native")]
mod visual;
pub mod watch;
pub use types::*;
