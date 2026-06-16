//! Plugin platform: manifest discovery, process lifecycle, and the host that
//! drives the JSON-RPC channel to each plugin.

pub mod host;
pub mod manifest;
pub mod process;

pub use host::{HostSink, PluginHost};
