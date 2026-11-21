//! Portable core: storage, queries, settings model, sampling and the UI protocol. No OS calls.

pub mod config;
pub mod fan_policy;
pub mod gpu;
pub mod history;
pub mod ipc;
pub mod presets;
pub mod sampler;
pub mod settings;
pub mod storage;
pub mod store;
pub mod time;
pub mod ui_state;
