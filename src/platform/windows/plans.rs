//! Power plan settings via PowrProf (the API behind `powercfg`). Hidden settings are writable here too.

use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, HLOCAL, LocalFree};
use windows::Win32::System::Power::{
    ACCESS_SCHEME, PowerEnumerate, PowerGetActiveScheme, PowerReadACDefaultIndex, PowerReadACValueIndex, PowerReadDCDefaultIndex,
    PowerReadDCValueIndex, PowerReadFriendlyName, PowerSetActiveScheme, PowerWriteACValueIndex, PowerWriteDCValueIndex,
};
use windows::core::GUID;

use crate::error::{Result, win32};
use crate::platform::{AcDc, Knob};

/// Stock Balanced plan: source of shipped defaults.
const BALANCED: GUID = GUID::from_u128(0x381b4222_f694_41f0_9685_ff5bb260df2e);
const SUB_PROCESSOR: GUID = GUID::from_u128(0x54533251_82be_4824_96c1_47b60b740d00);

#[derive(Clone, Copy)]
pub struct Setting {
    subgroup: GUID,
    guid: GUID,
}

pub fn setting(k: Knob) -> Setting {
    let guid = match k {
        Knob::ProcessorMax => 0xbc5038f7_23e0_4960_96da_33abaf5935ec, // PROCTHROTTLEMAX
        Knob::ProcessorMin => 0x893dee8e_2bef_41e0_89c6_b55d0929964c, // PROCTHROTTLEMIN
        Knob::BoostMode => 0xbe337238_0d82_4146_a960_4f3749d470c7,    // PERFBOOSTMODE
        Knob::EnergyPreference => 0x36687f9e_e3a5_4dbf_b1dc_15eb381c6863, // PERFEPP
        Knob::CoolingPolicy => 0x94d3a615_a899_4ac5_ae2b_e4d8f634367f, // SYSCOOLPOL
    };
    Setting { subgroup: SUB_PROCESSOR, guid: GUID::from_u128(guid) }
}

pub fn active_scheme() -> Result<GUID> {
    unsafe {
        let mut p: *mut GUID = std::ptr::null_mut();
        win32(PowerGetActiveScheme(None, &mut p).0, "PowerGetActiveScheme")?;
        let g = *p;
        LocalFree(Some(HLOCAL(p.cast())));
        Ok(g)
    }
}

fn schemes() -> Result<Vec<GUID>> {
    let mut out = Vec::new();
    for index in 0.. {
        let mut g = GUID::zeroed();
        let mut size = std::mem::size_of::<GUID>() as u32;
        match unsafe { PowerEnumerate(None, None, None, ACCESS_SCHEME, index, Some((&raw mut g).cast()), &mut size) } {
            ERROR_SUCCESS => out.push(g),
            ERROR_NO_MORE_ITEMS => break,
            e => win32(e.0, "PowerEnumerate")?,
        }
    }
    Ok(out)
}

pub fn scheme_name(scheme: &GUID) -> Result<String> {
    unsafe {
        let mut size = 0u32;
        win32(PowerReadFriendlyName(None, Some(scheme), None, None, None, &mut size).0, "PowerReadFriendlyName")?;
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        win32(PowerReadFriendlyName(None, Some(scheme), None, None, Some(buf.as_mut_ptr().cast()), &mut size).0, "PowerReadFriendlyName")?;
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Ok(String::from_utf16_lossy(&buf[..end]))
    }
}

pub fn read(scheme: &GUID, s: Setting) -> Result<AcDc> {
    let (mut ac, mut dc) = (0u32, 0u32);
    unsafe {
        win32(PowerReadACValueIndex(None, Some(scheme), Some(&s.subgroup), Some(&s.guid), &mut ac).0, "PowerReadACValueIndex")?;
        win32(PowerReadDCValueIndex(None, Some(scheme), Some(&s.subgroup), Some(&s.guid), &mut dc), "PowerReadDCValueIndex")?;
    }
    Ok(AcDc { ac, dc })
}

pub fn default(s: Setting) -> Result<AcDc> {
    let (mut ac, mut dc) = (0u32, 0u32);
    unsafe {
        win32(PowerReadACDefaultIndex(None, &BALANCED, Some(&s.subgroup), &s.guid, &mut ac), "PowerReadACDefaultIndex")?;
        win32(PowerReadDCDefaultIndex(None, &BALANCED, Some(&s.subgroup), &s.guid, &mut dc), "PowerReadDCDefaultIndex")?;
    }
    Ok(AcDc { ac, dc })
}

/// Every plan, mains and battery; re-applying the active plan makes it take effect now.
pub fn write_everywhere(s: Setting, value: u32) -> Result<()> {
    for scheme in schemes()? {
        unsafe {
            win32(PowerWriteACValueIndex(None, &scheme, Some(&s.subgroup), Some(&s.guid), value).0, "PowerWriteACValueIndex")?;
            win32(PowerWriteDCValueIndex(None, &scheme, Some(&s.subgroup), Some(&s.guid), value), "PowerWriteDCValueIndex")?;
        }
    }
    let active = active_scheme()?;
    win32(unsafe { PowerSetActiveScheme(None, Some(&active)) }.0, "PowerSetActiveScheme")
}
