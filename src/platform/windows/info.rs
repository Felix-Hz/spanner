//! Hardware description from WMI: `ROOT\CIMV2` for the machine, `ROOT\WMI` for battery wear and rate.

use super::com::Var;
use super::wmi::Wmi;
use crate::platform::{BatteryHealth, Display, Drive, Graphics, Memory, Os, Processor, SystemInfo};

fn text(v: &Var) -> Option<String> {
    v.to_string_opt().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// "12th Gen Intel(R) Core(TM) i7-1260P" to "12th Gen Intel Core i7-1260P".
fn plain(name: &str) -> String {
    let s = name.replace("(R)", "").replace("(TM)", "").replace("(tm)", "");
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The maker as people say it: "Micro-Star International Co., Ltd." is "MSI".
fn maker(raw: &str) -> String {
    const SHORT: &[(&str, &str)] = &[
        ("micro-star", "MSI"),
        ("hewlett-packard", "HP"),
        ("asustek", "ASUS"),
        ("lenovo", "Lenovo"),
        ("dell", "Dell"),
        ("acer", "Acer"),
        ("gigabyte", "Gigabyte"),
        ("razer", "Razer"),
        ("microsoft", "Microsoft"),
    ];
    let lower = raw.to_lowercase();
    SHORT.iter().find(|(k, _)| lower.starts_with(k)).map_or_else(|| raw.to_string(), |(_, v)| (*v).to_string())
}

fn int(v: &Var) -> Option<i64> {
    v.to_i64_opt()
}

fn u32_of(v: &Var) -> Option<u32> {
    int(v).and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0)
}

/// SMBIOS 3.x memory type codes (DSP0134, type 17 "Memory Type").
fn memory_kind(code: i64) -> Option<&'static str> {
    Some(match code {
        20 => "DDR",
        21 => "DDR2",
        24 => "DDR3",
        26 => "DDR4",
        27 => "LPDDR",
        28 => "LPDDR2",
        29 => "LPDDR3",
        30 => "LPDDR4",
        34 => "DDR5",
        35 => "LPDDR5",
        _ => return None,
    })
}

/// CIM datetime "20261121093012.500000+780" to Unix milliseconds, honouring the minute offset.
fn cim_time_ms(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (n(0, 4)?, n(4, 6)?, n(6, 8)?, n(8, 10)?, n(10, 12)?, n(12, 14)?);
    let offset_min = s.get(21..25).and_then(|o| o.parse::<i64>().ok()).unwrap_or(0);
    // Days from civil (Howard Hinnant's algorithm), valid for the Gregorian calendar.
    let (y2, m2) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400 + h * 3600 + mi * 60 + se) - offset_min * 60) * 1000)
}

pub fn read(cim: &Wmi, root_wmi: Option<&Wmi>) -> SystemInfo {
    let mut info = SystemInfo::default();

    if let Ok(rows) = cim.rows("Win32_ComputerSystem", &["Manufacturer", "Model"], None)
        && let Some(r) = rows.first()
    {
        info.model = match (text(&r[0]).map(|m| maker(&m)), text(&r[1])) {
            (Some(m), Some(model)) if !model.starts_with(&m) => Some(format!("{m} {model}")),
            (_, Some(model)) => Some(model),
            (m, None) => m,
        };
    }

    if let Ok(rows) = cim.rows("Win32_Processor", &["Name", "NumberOfCores", "NumberOfLogicalProcessors", "MaxClockSpeed"], None)
        && let Some(r) = rows.first()
        && let Some(name) = text(&r[0])
    {
        info.processor = Some(Processor { name: plain(&name), cores: u32_of(&r[1]), threads: u32_of(&r[2]), base_mhz: u32_of(&r[3]) });
    }

    if let Ok(rows) = cim.rows("Win32_PhysicalMemory", &["Capacity", "SMBIOSMemoryType", "Speed", "ConfiguredClockSpeed"], None) {
        let modules_mb: Vec<u64> = rows.iter().filter_map(|r| int(&r[0])).map(|b| (b as u64) / (1024 * 1024)).collect();
        if !modules_mb.is_empty() {
            info.memory = Some(Memory {
                total_mb: modules_mb.iter().sum(),
                kind: rows.iter().find_map(|r| int(&r[1]).and_then(memory_kind)).map(str::to_string),
                speed_mts: rows.iter().find_map(|r| u32_of(&r[2]).or_else(|| u32_of(&r[3]))),
                modules_mb,
            });
        }
    }

    if let Ok(rows) = cim.rows("Win32_VideoController", &["Name", "DriverVersion", "CurrentHorizontalResolution", "CurrentVerticalResolution", "CurrentRefreshRate"], None) {
        for r in &rows {
            if let Some(name) = text(&r[0]) {
                info.graphics.push(Graphics { name: plain(&name), driver: text(&r[1]), vram_mb: None });
            }
            if let (Some(width), Some(height)) = (u32_of(&r[2]), u32_of(&r[3])) {
                info.displays.push(Display { width, height, hz: u32_of(&r[4]) });
            }
        }
    }

    if let Ok(rows) = cim.rows("Win32_DiskDrive", &["Model", "Size", "MediaType"], None) {
        for r in &rows {
            let fixed = text(&r[2]).is_none_or(|m| m.to_lowercase().contains("fixed"));
            if let (Some(model), Some(bytes), true) = (text(&r[0]), int(&r[1]), fixed) {
                info.drives.push(Drive { model, size_gb: (bytes as u64) / 1_000_000_000 });
            }
        }
    }

    if let Ok(rows) = cim.rows("Win32_OperatingSystem", &["Caption", "Version", "BuildNumber", "LastBootUpTime"], None)
        && let Some(r) = rows.first()
        && let Some(name) = text(&r[0])
    {
        info.os = Some(Os {
            name: name.trim_start_matches("Microsoft ").to_string(),
            version: text(&r[1]),
            build: text(&r[2]),
            boot_ms: text(&r[3]).as_deref().and_then(cim_time_ms),
        });
    }

    if let Ok(rows) = cim.rows("Win32_BIOS", &["SMBIOSBIOSVersion"], None)
        && let Some(r) = rows.first()
    {
        info.bios = text(&r[0]);
    }

    if let Some(w) = root_wmi {
        let first = |class: &str, field: &str| w.rows(class, &[field], None).ok().and_then(|rows| rows.first().and_then(|r| u32_of(&r[0])));
        if let (Some(design_mwh), Some(full_mwh)) = (first("BatteryStaticData", "DesignedCapacity"), first("BatteryFullChargedCapacity", "FullChargedCapacity")) {
            info.battery = Some(BatteryHealth { design_mwh, full_mwh, cycles: first("BatteryCycleCount", "CycleCount") });
        }
    }

    info
}

/// Battery power in milliwatts from `ROOT\WMI:BatteryStatus`: positive charging, negative discharging.
pub fn battery_rate_mw(root_wmi: &Wmi) -> Option<i32> {
    let rows = root_wmi.rows("BatteryStatus", &["Charging", "Discharging", "ChargeRate", "DischargeRate"], None).ok()?;
    let r = rows.first()?;
    let charging = r[0].to_bool_opt().unwrap_or(false);
    let discharging = r[1].to_bool_opt().unwrap_or(false);
    if charging {
        int(&r[2]).and_then(|v| i32::try_from(v).ok())
    } else if discharging {
        int(&r[3]).and_then(|v| i32::try_from(v).ok()).map(|v| -v)
    } else {
        Some(0)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_cim_datetimes_with_offset() {
        // 2026-11-21 09:30:12 at UTC+13 (780 min) is 2026-11-20 20:30:12 UTC.
        assert_eq!(super::cim_time_ms("20261121093012.500000+780"), Some(1_795_206_612_000));
        assert_eq!(super::cim_time_ms("1970010100000.000000+000"), None, "too short");
    }

    #[test]
    fn tidies_names() {
        assert_eq!(super::plain("12th Gen Intel(R) Core(TM) i7-1260P"), "12th Gen Intel Core i7-1260P");
        assert_eq!(super::maker("Micro-Star International Co., Ltd."), "MSI");
        assert_eq!(super::maker("Framework"), "Framework");
    }

    #[test]
    fn maps_smbios_memory_types() {
        assert_eq!(super::memory_kind(26), Some("DDR4"));
        assert_eq!(super::memory_kind(34), Some("DDR5"));
        assert_eq!(super::memory_kind(2), None);
    }
}
