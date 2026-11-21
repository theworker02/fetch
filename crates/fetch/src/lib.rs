//! `fetch` is the Rust migration of aria2's download-facing core.
//!
//! This crate deliberately keeps protocol parsing, verification and control-plane
//! code separate so privileged file operations remain local and auditable.

pub mod daemon;
pub mod download;
pub mod metalink;
pub mod torrent;
pub mod transport;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
