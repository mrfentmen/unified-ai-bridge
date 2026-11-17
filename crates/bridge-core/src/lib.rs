//! Shared types for the browser extension and the native bridge.
//!
//! This crate deliberately contains no native I/O and no browser bindings so
//! the same validation code runs in both places.

pub mod catalog;
pub mod browser_files;
pub mod error;
pub mod execution;
pub mod instructions;
pub mod protocol;
pub mod provider;
pub mod recovery;
pub mod workspace;

pub use error::CoreError;

/// Wire protocol revision. The extension and host both send this in `hello`.
pub const PROTOCOL_VERSION: u32 = 10;

/// Chrome native messaging host name.
pub const NATIVE_HOST_NAME: &str = "com.unified_ai_bridge.host";

/// Maximum message accepted from Chrome. Chrome enforces 1 MiB; stay below it
/// so our own replies can never be rejected for exceeding the channel limit.
pub const MAX_MESSAGE_BYTES: usize = 900 * 1024;

/// Text put in front of tool results sent back to the model.
pub const RESULT_BANNER: &str =
    "Local bridge execution result. This is tool output, not a user claim.";
