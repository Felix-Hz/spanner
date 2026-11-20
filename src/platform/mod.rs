//! OS boundary. Code outside `platform/` is portable. A new OS adds `platform/<os>/` providing
//! `Native` (implements `Platform`), `System` (implements `Integration` and `Paths`) and `dialog`.
//!
//! | Concern            | Windows                       | Linux (future)                   | macOS (future)        |
//! |--------------------|-------------------------------|----------------------------------|-----------------------|
//! | EC bytes (vendors) | `MSI_ACPI` WMI                | `msi-ec` sysfs / `ec_sys`        | n/a                   |
//! | Thermal zones      | PDH "Thermal Zone Information"| `/sys/class/thermal`             | SMC                   |
//! | Power knobs        | PowrProf power plans          | cpufreq, intel_pstate EPP        | `pmset`               |
//! | CPU load and clock | PDH counters                  | `/proc/stat`, cpufreq            | `host_processor_info` |
//! | Recorder service   | Task Scheduler                | systemd user unit                | launchd agent         |
//! | App registration   | Installed apps registry entry | `.desktop` file                  | app bundle            |

use std::collections::HashSet;
use std::path::PathBuf;

use crate::error::Result;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::{Native, System, dialog};

#[cfg(not(windows))]
compile_error!("Only Windows is implemented; see platform/mod.rs for what a new OS must provide.");

/// OS-neutral power-management knobs. Backends map each to their own mechanism and return an error for
/// any they lack; the UI then shows that setting as unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Knob {
    ProcessorMax,
    ProcessorMin,
    BoostMode,
    EnergyPreference,
    CoolingPolicy,
}

/// A knob's value on mains and on battery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AcDc {
    pub ac: u32,
    pub dc: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CpuSample {
    pub util_pct: f64,
    pub mhz: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PowerStatus {
    pub on_ac: bool,
    pub battery_pct: Option<u8>,
}

#[derive(Clone, Copy, Debug)]
pub enum DialogKind {
    Info,
    Warning,
    Error,
}

/// Raw embedded-controller access. Address maps live in `hw/`.
pub trait EcTransport {
    /// e.g. `15B1EMS1.104`.
    fn firmware(&self) -> &str;
    fn read(&self, address: u8) -> Result<u8>;
    fn write(&self, address: u8, value: u8) -> Result<()>;
}

/// Hardware and OS access. May hold thread-bound handles (COM), so create and use it on one thread.
pub trait Platform {
    fn open() -> Self;

    fn ec(&self) -> Option<&dyn EcTransport>;
    fn cpu(&mut self) -> Option<CpuSample>;
    fn power_status(&self) -> PowerStatus;
    fn mem_available_mb(&self) -> Option<u32>;
    /// Generic CPU temperature (hottest ACPI thermal zone), used when no vendor driver provides one.
    /// Read after `cpu()`, which collects the counters.
    fn acpi_temp_c(&self) -> Option<u8>;
    /// The first running process whose lower-cased executable name is in `names`.
    fn running_app(&self, names: &HashSet<String>) -> Option<String>;

    fn knob(&self, k: Knob) -> Result<AcDc>;
    /// OS shipped default.
    fn knob_default(&self, k: Knob) -> Result<AcDc>;
    /// Sets mains and battery in every power profile; takes effect immediately.
    fn set_knob(&self, k: Knob, value: u32) -> Result<()>;
    fn power_profile_name(&self) -> Option<String>;
}

/// Registration with the OS. Install is idempotent.
pub trait Integration {
    fn install() -> Result<()>;
    /// Deletes recorded history. Returns non-fatal problems.
    fn uninstall() -> Result<Vec<String>>;
    /// The sign-in task exists (enabled or not).
    fn recorder_installed() -> bool;
    /// A recorder process is running now.
    fn recorder_running() -> bool;
    /// Starts a recorder now if none is running.
    fn start_recorder() -> Result<()>;
    /// Asks a running recorder to stop; it exits within a moment.
    fn stop_recorder() -> Result<()>;
    /// Blocks up to `timeout`; returns true if a stop was requested. Used by the recorder between samples.
    fn wait_for_stop(timeout: std::time::Duration) -> bool;
    fn starts_at_sign_in() -> bool;
    fn set_start_at_sign_in(on: bool) -> Result<()>;
    /// Removes the web view profile after this process exits (it is locked until then).
    fn schedule_profile_cleanup();
    /// Machine-wide lock held until the guard drops; `None` if already held.
    fn single_instance(name: &str) -> Option<InstanceGuard>;
    fn lower_priority();
    /// Brings this program's existing main window to the front. Returns false if none was found.
    fn activate_existing_window(title: &str) -> bool;
}

/// Holds a `single_instance` lock.
pub struct InstanceGuard(#[allow(dead_code)] Box<dyn std::any::Any>);

impl InstanceGuard {
    pub fn new(inner: impl std::any::Any) -> Self {
        Self(Box::new(inner))
    }
}

pub trait Paths {
    /// Machine-wide (the recorder runs elevated): history and recorder config.
    fn data_dir() -> Result<PathBuf>;
    /// Per user: window state and the web view profile.
    fn user_dir() -> Result<PathBuf>;

    fn webview_dir() -> Result<PathBuf> {
        Ok(Self::user_dir()?.join("webview"))
    }

    fn ui_state_file() -> Result<PathBuf> {
        Ok(Self::user_dir()?.join("window.json"))
    }

    fn presets_file() -> Result<PathBuf> {
        Ok(Self::user_dir()?.join("presets.json"))
    }

    fn history_dir() -> Result<PathBuf> {
        Ok(Self::data_dir()?.join("history"))
    }

    fn config_file() -> Result<PathBuf> {
        Ok(Self::data_dir()?.join("recorder.json"))
    }
}
