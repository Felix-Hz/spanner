//! Windows backend.

mod com;
mod ec;
mod info;
mod install;
mod paths;
mod pipe;
mod plans;
mod sensors;
mod window;
mod wmi;

use std::collections::HashSet;

use windows::Win32::System::Com::COINIT_MULTITHREADED;
use windows::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_OKCANCEL, MessageBoxW};
use windows::core::HSTRING;

use super::{AcDc, CpuSample, DialogKind, EcTransport, Knob, Platform, PowerStatus, SystemInfo};
use crate::error::Result;

pub use install::System;

pub struct Native {
    ec: Option<ec::WmiEc>,
    cpu: Option<sensors::Cpu>,
    cim: Option<wmi::Wmi>,
    root_wmi: Option<wmi::Wmi>,
    // Last: COM must outlive the WMI handles above (fields drop in declaration order).
    _com: com::Com,
}

impl Platform for Native {
    fn open() -> Self {
        let com = com::Com::init(COINIT_MULTITHREADED);
        Self {
            ec: ec::WmiEc::open().ok(),
            cpu: sensors::Cpu::open().ok(),
            cim: wmi::Wmi::connect(r"ROOT\CIMV2").ok(),
            root_wmi: wmi::Wmi::connect(r"ROOT\WMI").ok(),
            _com: com,
        }
    }

    fn ec(&self) -> Option<&dyn EcTransport> {
        self.ec.as_ref().map(|e| e as &dyn EcTransport)
    }

    fn cpu(&mut self) -> Option<CpuSample> {
        self.cpu.as_ref().and_then(sensors::Cpu::sample)
    }

    fn power_status(&self) -> PowerStatus {
        sensors::power_status()
    }

    fn mem_available_mb(&self) -> Option<u32> {
        sensors::mem_available_mb()
    }

    fn mem_total_mb(&self) -> Option<u32> {
        sensors::mem_total_mb()
    }

    fn disk_active_pct(&self) -> Option<f64> {
        self.cpu.as_ref().and_then(sensors::Cpu::disk_active_pct)
    }

    fn battery_rate_mw(&self) -> Option<i32> {
        self.root_wmi.as_ref().and_then(info::battery_rate_mw)
    }

    fn system_info(&self) -> SystemInfo {
        self.cim.as_ref().map(|c| info::read(c, self.root_wmi.as_ref())).unwrap_or_default()
    }

    fn acpi_temp_c(&self) -> Option<u8> {
        self.cpu.as_ref().and_then(sensors::Cpu::acpi_temp_c)
    }

    fn running_app(&self, names: &HashSet<String>) -> Option<String> {
        sensors::running_app(names)
    }

    fn knob(&self, k: Knob) -> Result<AcDc> {
        plans::read(&plans::active_scheme()?, plans::setting(k))
    }

    fn knob_default(&self, k: Knob) -> Result<AcDc> {
        plans::default(plans::setting(k))
    }

    fn set_knob(&self, k: Knob, value: u32) -> Result<()> {
        plans::write_everywhere(plans::setting(k), value)
    }

    fn power_profile_name(&self) -> Option<String> {
        plans::active_scheme().and_then(|g| plans::scheme_name(&g)).ok()
    }
}

/// Native message box. Returns true for OK.
pub fn dialog(text: &str, kind: DialogKind, confirm: bool) -> bool {
    let icon = match kind {
        DialogKind::Info => MB_ICONINFORMATION,
        DialogKind::Warning => MB_ICONWARNING,
        DialogKind::Error => MB_ICONERROR,
    };
    let buttons = if confirm { MB_OKCANCEL } else { MB_OK };
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from("spanner"), buttons | icon) == IDOK }
}
