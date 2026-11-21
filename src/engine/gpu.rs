//! Nvidia GPU via NVML (`nvml.dll` / `libnvidia-ml.so`), loaded at runtime; absent drivers mean no GPU data.

use std::time::{Duration, Instant};

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};

#[derive(Clone, Copy, Debug, Default)]
pub struct GpuSample {
    pub temp_c: u32,
    pub util_pct: u32,
    pub power_w: f64,
    pub mhz: u32,
    pub vram_mb: u64,
    /// NVML clock-event reason bits.
    pub throttle: u64,
}

/// Lazy NVML handle; init is retried at most once a minute (the driver may load after logon).
pub struct Gpu {
    nvml: Option<Nvml>,
    next_try: Instant,
}

impl Default for Gpu {
    fn default() -> Self {
        Self { nvml: None, next_try: Instant::now() }
    }
}

impl Gpu {
    /// Name and total memory of the first NVML device, for the hardware description.
    pub fn describe(&mut self) -> Option<(String, u64)> {
        self.sample()?;
        let dev = self.nvml.as_ref()?.device_by_index(0).ok()?;
        Some((dev.name().ok()?, dev.memory_info().ok()?.total / (1024 * 1024)))
    }

    pub fn sample(&mut self) -> Option<GpuSample> {
        if self.nvml.is_none() && Instant::now() >= self.next_try {
            self.nvml = Nvml::init().ok();
            self.next_try = Instant::now() + Duration::from_secs(60);
        }
        let dev = self.nvml.as_ref()?.device_by_index(0).ok()?;
        Some(GpuSample {
            temp_c: dev.temperature(TemperatureSensor::Gpu).ok()?,
            util_pct: dev.utilization_rates().map_or(0, |u| u.gpu),
            power_w: dev.power_usage().map_or(0.0, |mw| f64::from(mw) / 1000.0),
            mhz: dev.clock_info(Clock::Graphics).unwrap_or(0),
            vram_mb: dev.memory_info().map_or(0, |m| m.used / (1024 * 1024)),
            throttle: dev.current_throttle_reasons().map_or(0, |r| r.bits()),
        })
    }
}
