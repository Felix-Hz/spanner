//! CPU counters (PDH), power status, memory and the process list.

use std::collections::HashSet;

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Performance::{
    PDH_FMT, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW,
    PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::core::{HSTRING, PCWSTR};

use crate::error::{Result, win32};
use crate::platform::{CpuSample, PowerStatus};

/// PDH_FMT_NOCAP100: "% Processor Performance" exceeds 100 while boosting.
const PDH_FMT_NOCAP100: PDH_FMT = PDH_FMT(0x0000_8000);

pub struct Cpu {
    query: PDH_HQUERY,
    utility: PDH_HCOUNTER,
    performance: PDH_HCOUNTER,
    frequency: PDH_HCOUNTER,
    /// All ACPI thermal zones; fallback CPU temperature when no vendor driver reads one.
    thermal: Option<PDH_HCOUNTER>,
    /// Idle share of all physical disks; busy is its complement ("% Disk Time" exceeds 100 with queues).
    disk_idle: Option<PDH_HCOUNTER>,
}

const PDH_MORE_DATA: u32 = 0x8000_07D2;

impl Cpu {
    pub fn open() -> Result<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            win32(PdhOpenQueryW(PCWSTR::null(), 0, &mut query), "PdhOpenQueryW")?;
            let add = |path: &str| -> Result<PDH_HCOUNTER> {
                let mut c = PDH_HCOUNTER::default();
                win32(PdhAddEnglishCounterW(query, &HSTRING::from(path), 0, &mut c), "PdhAddEnglishCounterW")?;
                Ok(c)
            };
            let cpu = Self {
                utility: add(r"\Processor Information(_Total)\% Processor Utility")?,
                performance: add(r"\Processor Information(_Total)\% Processor Performance")?,
                frequency: add(r"\Processor Information(_Total)\Processor Frequency")?,
                thermal: add(r"\Thermal Zone Information(*)\Temperature").ok(),
                disk_idle: add(r"\PhysicalDisk(_Total)\% Idle Time").ok(),
                query,
            };
            // Rate counters diff against the previous collection.
            PdhCollectQueryData(query);
            Ok(cpu)
        }
    }

    fn value(c: PDH_HCOUNTER) -> Option<f64> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        let rc = unsafe { PdhGetFormattedCounterValue(c, PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100.0), None, &mut v) };
        (rc == 0).then_some(unsafe { v.Anonymous.doubleValue })
    }

    pub fn sample(&self) -> Option<CpuSample> {
        unsafe { PdhCollectQueryData(self.query) };
        let util = Self::value(self.utility)?;
        // "Processor Frequency" is nominal; × "% Processor Performance" gives effective.
        let mhz = Self::value(self.frequency)? * Self::value(self.performance)? / 100.0;
        Some(CpuSample { util_pct: util.clamp(0.0, 100.0), mhz })
    }
}

impl Cpu {
    /// Busy share of all physical disks, from the last `sample()` collection.
    pub fn disk_active_pct(&self) -> Option<f64> {
        Self::value(self.disk_idle?).map(|idle| (100.0 - idle).clamp(0.0, 100.0))
    }

    /// Hottest ACPI thermal zone, from the last `sample()` collection. Zones report Kelvin.
    pub fn acpi_temp_c(&self) -> Option<u8> {
        let counter = self.thermal?;
        let (mut bytes, mut count) = (0u32, 0u32);
        let rc = unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, None) };
        if rc != PDH_MORE_DATA {
            return None;
        }
        // Items are followed by their name strings in the same buffer.
        let n = (bytes as usize).div_ceil(std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>());
        let mut buf = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); n];
        let rc = unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, Some(buf.as_mut_ptr())) };
        if rc != 0 {
            return None;
        }
        buf[..count as usize]
            .iter()
            .filter(|i| i.FmtValue.CStatus == 0)
            .map(|i| unsafe { i.FmtValue.Anonymous.doubleValue } - 273.15)
            .filter(|c| (1.0..120.0).contains(c))
            .max_by(f64::total_cmp)
            .map(|c| c.round() as u8)
    }
}

impl Drop for Cpu {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

pub fn power_status() -> PowerStatus {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() {
        return PowerStatus::default();
    }
    // BatteryLifePercent is 255 when unknown.
    PowerStatus { on_ac: s.ACLineStatus == 1, battery_pct: (s.BatteryLifePercent <= 100).then_some(s.BatteryLifePercent) }
}

fn memory_status() -> Option<MEMORYSTATUSEX> {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut m) }.ok()?;
    Some(m)
}

pub fn mem_available_mb() -> Option<u32> {
    u32::try_from(memory_status()?.ullAvailPhys / (1024 * 1024)).ok()
}

pub fn mem_total_mb() -> Option<u32> {
    u32::try_from(memory_status()?.ullTotalPhys / (1024 * 1024)).ok()
}

pub fn running_app(names: &HashSet<String>) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return None };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = None;
        let mut more = Process32FirstW(snap, &mut e).is_ok();
        while more && found.is_none() {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase();
            found = names.contains(&name).then_some(name);
            more = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
        found
    }
}
