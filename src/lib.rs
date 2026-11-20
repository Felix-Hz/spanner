//! spanner: power, cooling and battery tuning with recorded monitoring for Windows laptops.
//!
//! The binary (`main.rs`) only parses arguments; everything else lives here, so unit tests run
//! without the binary's administrator manifest.

pub mod app;
pub mod assets;
pub mod engine;
pub mod error;
pub mod hw;
pub mod platform;
pub mod recorder;
