//! Vendor drivers: machine-specific controls and sensors (fans, performance modes, charge limits) that
//! have no OS-level API. Each driver detects its hardware through the transports in `Access`; the first
//! match wins. Machines with no driver still get every generic setting and sensor.
//!
//! Adding a vendor: implement `Vendor`, add its `detect` to `DRIVERS`.

pub mod msi;

use crate::engine::settings::Def;
use crate::error::Result;
use crate::platform::EcTransport;

/// Low-level transports a driver may use. Borrowed per call from the platform, which owns them.
#[derive(Clone, Copy)]
pub struct Access<'a> {
    pub ec: Option<&'a dyn EcTransport>,
}

/// One labelled vendor state for the Live view, e.g. ("Fan profile", "Auto").
pub type Status = (&'static str, String);

#[derive(Clone, Debug, Default)]
pub struct Readings {
    pub cpu_temp_c: Option<u8>,
    /// `None` while the dGPU is powered down, where the vendor can tell.
    pub gpu_temp_c: Option<u8>,
    /// 0 while the fan is stopped.
    pub cpu_fan_rpm: Option<u16>,
    pub gpu_fan_rpm: Option<u16>,
    /// Fans forced to maximum (MSI Cooler Boost and equivalents).
    pub max_cooling: Option<bool>,
    pub status: Vec<Status>,
}

pub trait Vendor: Send {
    /// Human-readable machine and firmware, e.g. "MSI 15B1EMS1.104".
    fn describe(&self) -> String;
    fn readings(&self, a: Access) -> Readings;
    /// Settings this driver adds; their `Target::Vendor` ids are passed back to `read` and `write`.
    fn settings(&self) -> &'static [Def];
    fn read(&self, a: Access, id: &str) -> Option<i64>;
    /// `value` has already been validated against the setting's `Def`.
    fn write(&self, a: Access, id: &str, value: i64) -> Result<()>;

    /// Whether the machine can force its fans to maximum (MSI Cooler Boost and equivalents).
    /// Drivers that can enable the generic "Full-speed fans" setting and its "When hot" mode.
    fn supports_max_cooling(&self) -> bool {
        false
    }

    fn set_max_cooling(&self, _a: Access, _on: bool) -> Result<()> {
        Err(crate::error::Error::msg("This machine can't force its fans to full speed."))
    }
}

type Detect = fn(Access) -> Option<Box<dyn Vendor>>;

const DRIVERS: &[Detect] = &[msi::detect];

pub fn detect(a: Access) -> Option<Box<dyn Vendor>> {
    DRIVERS.iter().find_map(|d| d(a))
}
