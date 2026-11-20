//! One sample from every source: a `Record` to store and a `Live` view to show.

use std::collections::HashSet;

use serde::Serialize;

use super::config::app_id;
use super::gpu::{Gpu, GpuSample};
use super::history::GPU_LIMIT_MASK;
use super::store::{Record, flag};
use super::time::now_ms;
use crate::hw::{self, Access, Readings, Vendor};
use crate::platform::Platform;

pub struct Sampler<P: Platform> {
    pub platform: P,
    pub vendor: Option<Box<dyn Vendor>>,
    gpu: Gpu,
    pub tracked: HashSet<String>,
}

pub struct Sample {
    pub record: Record,
    pub vendor: Readings,
    pub gpu: Option<GpuSample>,
}

impl<P: Platform> Sampler<P> {
    /// Create and use on one thread (see `Platform`).
    pub fn new(tracked: HashSet<String>) -> Self {
        let platform = P::open();
        let vendor = hw::detect(Access { ec: platform.ec() });
        Self { platform, vendor, gpu: Gpu::default(), tracked }
    }

    pub fn vendor(&self) -> Option<&dyn Vendor> {
        self.vendor.as_deref()
    }

    pub fn sample(&mut self) -> Sample {
        let vendor = self.vendor.as_ref().map(|v| v.readings(Access { ec: self.platform.ec() })).unwrap_or_default();
        let p = &mut self.platform;
        let power = p.power_status();
        // NVML wakes a sleeping dGPU. On battery, query it only if the vendor shows it already powered;
        // without a vendor driver there is no way to tell, so it is skipped on battery.
        let gpu = if power.on_ac || vendor.gpu_temp_c.is_some() { self.gpu.sample() } else { None };
        let cpu = p.cpu();
        let cpu_temp = vendor.cpu_temp_c.or_else(|| p.acpi_temp_c());

        let mut flags = 0;
        if power.on_ac {
            flags |= flag::ON_AC;
        }
        let app = p.running_app(&self.tracked);
        if app.is_some() {
            flags |= flag::TRACKED_APP;
        }
        if vendor.max_cooling == Some(true) {
            flags |= flag::MAX_COOLING;
        }
        let u16_of = |v: f64| Some(v.round().clamp(0.0, f64::from(u16::MAX - 1)) as u16);
        let record = Record {
            ts_ms: now_ms(),
            cpu_temp_c: cpu_temp,
            gpu_temp_c: gpu.map(|g| g.temp_c.min(254) as u8).or(vendor.gpu_temp_c),
            gpu_util_pct: gpu.map(|g| g.util_pct.min(100) as u8),
            battery_pct: power.battery_pct,
            flags,
            cpu_util_x10: cpu.and_then(|c| u16_of(c.util_pct * 10.0)),
            cpu_mhz: cpu.and_then(|c| u16_of(c.mhz)),
            gpu_power_dw: gpu.and_then(|g| u16_of(g.power_w * 10.0)),
            gpu_mhz: gpu.and_then(|g| u16_of(f64::from(g.mhz))),
            gpu_vram_mb: gpu.and_then(|g| u16_of(g.vram_mb as f64)),
            gpu_throttle: gpu.map(|g| (g.throttle & 0xFFFF) as u16),
            cpu_fan_rpm: vendor.cpu_fan_rpm,
            gpu_fan_rpm: vendor.gpu_fan_rpm,
            mem_avail_mb: p.mem_available_mb(),
            app: app.as_deref().map_or(0, app_id),
        };
        Sample { record, vendor, gpu }
    }
}

#[derive(Serialize)]
pub struct Live {
    pub ts_ms: i64,
    pub cpu_temp_c: Option<u8>,
    pub cpu_util_pct: Option<f32>,
    pub cpu_mhz: Option<u16>,
    pub gpu_temp_c: Option<u8>,
    pub gpu_util_pct: Option<u8>,
    pub gpu_power_w: Option<f32>,
    pub gpu_mhz: Option<u16>,
    pub gpu_vram_mb: Option<u16>,
    /// No NVML reading: dGPU powered down, or no driver.
    pub gpu_asleep: bool,
    pub gpu_limits: Vec<&'static str>,
    pub cpu_fan_rpm: Option<u16>,
    pub gpu_fan_rpm: Option<u16>,
    pub max_cooling: Option<bool>,
    /// Vendor states, e.g. [("Fan profile", "Auto")].
    pub status: Vec<hw::Status>,
    pub on_ac: bool,
    pub battery_pct: Option<u8>,
    pub mem_avail_mb: Option<u32>,
    pub tracked_app: bool,
}

/// NVML clock-event reasons that indicate a limit, most significant first.
const LIMITS: [(u16, &str); 5] =
    [(0x40, "hardware thermal"), (0x20, "thermal"), (0x04, "power cap"), (0x80, "power brake"), (0x08, "hardware slowdown")];

fn limits(bits: u16) -> Vec<&'static str> {
    LIMITS.iter().filter(|(b, _)| bits & b & GPU_LIMIT_MASK != 0).map(|(_, n)| *n).collect()
}

impl From<Sample> for Live {
    fn from(s: Sample) -> Self {
        let r = s.record;
        Self {
            ts_ms: r.ts_ms,
            cpu_temp_c: r.cpu_temp_c,
            cpu_util_pct: r.cpu_util_x10.map(|v| f32::from(v) / 10.0),
            cpu_mhz: r.cpu_mhz,
            gpu_temp_c: r.gpu_temp_c,
            gpu_util_pct: r.gpu_util_pct,
            gpu_power_w: r.gpu_power_dw.map(|v| f32::from(v) / 10.0),
            gpu_mhz: r.gpu_mhz,
            gpu_vram_mb: r.gpu_vram_mb,
            gpu_asleep: s.gpu.is_none(),
            gpu_limits: r.gpu_throttle.map(limits).unwrap_or_default(),
            cpu_fan_rpm: r.cpu_fan_rpm,
            gpu_fan_rpm: r.gpu_fan_rpm,
            max_cooling: s.vendor.max_cooling,
            status: s.vendor.status,
            on_ac: r.has(flag::ON_AC),
            battery_pct: r.battery_pct,
            mem_avail_mb: r.mem_avail_mb,
            tracked_app: r.has(flag::TRACKED_APP),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn decodes_throttle_bits() {
        assert_eq!(super::limits(0x24), ["thermal", "power cap"]);
        assert!(super::limits(0x01).is_empty(), "idle is not a limit");
    }
}
