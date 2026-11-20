//! MSI laptops via the embedded controller (EC).
//!
//! Address maps per firmware family, from msi-ec (github.com/BeardOverflow/msi-ec). A family is enabled
//! here only after it has been verified on real hardware: firmware string at 0xA0 and plausible mode,
//! fan and charge values. Writes are restricted to the settings below and confirmed by read-back.
//!
//! Fan speed: msi-ec's `rt_fan_speed` bytes (0x71/0x89) stay 0 on 15B1EMS1 at full speed; they sit in the
//! fan-curve table. The tachometer words at 0xC8/0xCA (big-endian, 0 when stopped) track the fans, verified
//! by toggling Cooler Boost. RPM = 480000 / raw, as in MControlCenter (src/operate.cpp, getFan1Speed).

use super::{Access, Readings, Vendor};
use crate::engine::settings::{Def, Kind, Opt, Section, Target};
use crate::error::{Error, Result};
use crate::platform::EcTransport;

/// Charge limit floor, so a typo cannot leave the battery barely charging.
pub const CHARGE_MIN: i64 = 50;

/// One firmware family's address map.
pub struct Conf {
    /// Firmware prefix, e.g. `15B1EMS1.`.
    family: &'static str,
    models: &'static str,
    cpu_temp: u8,
    gpu_temp: u8,
    /// High byte of each 16-bit tachometer word (CPU, GPU).
    fan_tach: (u8, u8),
    cooler_boost: (u8, u8),
    shift_mode: u8,
    /// Eco, Comfort, Turbo.
    shift_values: [u8; 3],
    fan_mode: u8,
    /// Auto, Silent, Advanced.
    fan_values: [u8; 3],
    /// Value is `0x80 + percent`.
    charge_limit: u8,
}

/// msi-ec `CONF_G2_10`.
const CONFS: &[Conf] = &[Conf {
    family: "15B1EMS1.",
    models: "Stealth 15M B12UE",
    cpu_temp: 0x68,
    gpu_temp: 0x80,
    fan_tach: (0xC8, 0xCA),
    cooler_boost: (0x98, 1 << 7),
    shift_mode: 0xD2,
    shift_values: [0xC2, 0xC1, 0xC4],
    fan_mode: 0xD4,
    fan_values: [0x0D, 0x1D, 0x8D],
    charge_limit: 0xD7,
}];

const CHARGE_BASE: u8 = 0x80;
const TACH_RPM: u32 = 480_000;

fn rpm(raw: u16) -> u16 {
    if raw == 0 { 0 } else { (TACH_RPM / u32::from(raw)).min(u32::from(u16::MAX - 1)) as u16 }
}

/// Values are the driver's mode indices; listed from quietest to strongest.
const POWER_MODES: &[Opt] = &[
    Opt { value: 0, label: "Eco", help: "Lowest power limits. Quietest and coolest, slowest in games." },
    Opt { value: 1, label: "Balanced", help: "MSI's default limits. MSI Center calls this Comfort." },
    Opt { value: 2, label: "Turbo", help: "Highest power limits. Fastest, and the hottest and loudest." },
];

const FAN_PROFILES: &[Opt] = &[
    Opt { value: 1, label: "Silent", help: "Fans stay quiet and the laptop runs warmer." },
    Opt { value: 0, label: "Auto", help: "MSI's standard fan curve." },
    Opt { value: 2, label: "Advanced", help: "Uses the custom fan curve set in MSI Center." },
];

fn label(opts: &[Opt], value: i64) -> Option<&'static str> {
    opts.iter().find(|o| o.value == value).map(|o| o.label)
}

const SETTINGS: &[Def] = &[
    Def {
        id: "msi.shift_mode",
        section: Section::Performance,
        advanced: false,
        name: "Power mode",
        help: "MSI's power limits for the CPU and GPU together.",
        kind: Kind::Choice(POWER_MODES),
        target: Target::Vendor,
        default: Some(1),
        gaming: Some(1),
    },
    Def {
        id: "msi.fan_mode",
        section: Section::Cooling,
        advanced: false,
        name: "Fan profile",
        help: "How the fans respond to temperature.",
        kind: Kind::Choice(FAN_PROFILES),
        target: Target::Vendor,
        default: Some(0),
        gaming: Some(2),
    },
    Def {
        id: "msi.charge_limit",
        section: Section::Battery,
        advanced: false,
        name: "Charge limit",
        help: "Charging stops at this level. A lower limit slows battery wear if the laptop is mostly plugged in. MSI Center can change this too.",
        kind: Kind::Number { min: CHARGE_MIN, max: 100, step: 5, unit: "%", ends: None },
        target: Target::Vendor,
        default: Some(100),
        gaming: None,
    },
];

pub struct Msi {
    conf: &'static Conf,
    firmware: String,
}

pub fn detect(a: Access) -> Option<Box<dyn Vendor>> {
    let ec = a.ec?;
    let conf = CONFS.iter().find(|c| ec.firmware().starts_with(c.family))?;
    Some(Box::new(Msi { conf, firmware: ec.firmware().to_string() }))
}

fn index_of(values: &[u8; 3], b: u8) -> Option<usize> {
    values.iter().position(|&v| v == b)
}

impl Msi {
    fn ec<'a>(&self, a: Access<'a>) -> Result<&'a dyn EcTransport> {
        a.ec.ok_or_else(|| Error::msg("MSI controller not reachable."))
    }
}

impl Vendor for Msi {
    fn describe(&self) -> String {
        format!("MSI {} · EC {}", self.conf.models, self.firmware)
    }

    fn readings(&self, a: Access) -> Readings {
        let Ok(ec) = self.ec(a) else { return Readings::default() };
        let c = self.conf;
        let byte = |addr| ec.read(addr).ok();
        let tach = |hi: u8| Some(rpm(u16::from_be_bytes([byte(hi)?, byte(hi + 1)?])));
        // The EC reports 0 for a powered-down sensor.
        let temp = |addr| byte(addr).filter(|t| (1..120).contains(t));
        let boost = byte(c.cooler_boost.0).map(|b| b & c.cooler_boost.1 != 0);
        let mut status = Vec::new();
        if let Some(l) = byte(c.fan_mode).and_then(|b| index_of(&c.fan_values, b)).and_then(|i| label(FAN_PROFILES, i as i64)) {
            status.push(("Fan profile", l.to_string()));
        }
        if let Some(l) = byte(c.shift_mode).and_then(|b| index_of(&c.shift_values, b)).and_then(|i| label(POWER_MODES, i as i64)) {
            status.push(("Power mode", l.to_string()));
        }
        if let Some(p) = self.read(a, "msi.charge_limit") {
            status.push(("Charge limit", format!("{p}%")));
        }
        Readings {
            cpu_temp_c: temp(c.cpu_temp),
            gpu_temp_c: temp(c.gpu_temp),
            cpu_fan_rpm: tach(c.fan_tach.0),
            gpu_fan_rpm: tach(c.fan_tach.1),
            max_cooling: boost,
            status,
        }
    }

    fn settings(&self) -> &'static [Def] {
        SETTINGS
    }

    fn read(&self, a: Access, id: &str) -> Option<i64> {
        let ec = self.ec(a).ok()?;
        let c = self.conf;
        let b = ec.read(match id {
            "msi.shift_mode" => c.shift_mode,
            "msi.fan_mode" => c.fan_mode,
            "msi.charge_limit" => c.charge_limit,
            _ => return None,
        });
        let b = b.ok()?;
        match id {
            "msi.shift_mode" => index_of(&c.shift_values, b).map(|i| i as i64),
            "msi.fan_mode" => index_of(&c.fan_values, b).map(|i| i as i64),
            _ => b.checked_sub(CHARGE_BASE).filter(|p| *p <= 100).map(i64::from),
        }
    }

    fn write(&self, a: Access, id: &str, value: i64) -> Result<()> {
        let ec = self.ec(a)?;
        let c = self.conf;
        let idx = || usize::try_from(value).ok().filter(|i| *i < 3).ok_or_else(|| Error(format!("{value} is out of range for {id}.")));
        match id {
            "msi.shift_mode" => ec.write(c.shift_mode, c.shift_values[idx()?]),
            "msi.fan_mode" => ec.write(c.fan_mode, c.fan_values[idx()?]),
            "msi.charge_limit" if (CHARGE_MIN..=100).contains(&value) => ec.write(c.charge_limit, CHARGE_BASE + value as u8),
            "msi.charge_limit" => Err(Error(format!("Charge limit {value}% is outside {CHARGE_MIN}–100%."))),
            _ => Err(Error(format!("Unknown MSI setting '{id}'."))),
        }
    }

    fn supports_max_cooling(&self) -> bool {
        true
    }

    /// Cooler Boost. Read-modify-write: the other bits of this byte belong to other features.
    fn set_max_cooling(&self, a: Access, on: bool) -> Result<()> {
        let ec = self.ec(a)?;
        let (addr, bit) = self.conf.cooler_boost;
        let cur = ec.read(addr)?;
        let next = if on { cur | bit } else { cur & !bit };
        if next == cur { Ok(()) } else { ec.write(addr, next) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeEc {
        mem: RefCell<[u8; 256]>,
        fw: &'static str,
    }

    impl FakeEc {
        /// Values read from the real machine on 2026-11-20.
        fn stealth() -> Self {
            let mut mem = [0u8; 256];
            mem[0x98] = 0x02;
            mem[0xD4] = 0x0D;
            mem[0xD2] = 0xC1;
            mem[0xD7] = 0xBC;
            mem[0x68] = 58;
            // Cooler Boost on: CPU 0x0053, GPU 0x004F.
            mem[0xC9] = 0x53;
            mem[0xCB] = 0x4F;
            Self { mem: RefCell::new(mem), fw: "15B1EMS1.104" }
        }
    }

    impl EcTransport for FakeEc {
        fn firmware(&self) -> &str {
            self.fw
        }
        fn read(&self, a: u8) -> Result<u8> {
            Ok(self.mem.borrow()[a as usize])
        }
        fn write(&self, a: u8, v: u8) -> Result<()> {
            self.mem.borrow_mut()[a as usize] = v;
            Ok(())
        }
    }

    fn access(ec: &FakeEc) -> Access<'_> {
        Access { ec: Some(ec) }
    }

    #[test]
    fn decodes_the_real_machine() {
        let ec = FakeEc::stealth();
        let a = access(&ec);
        let v = detect(a).unwrap();
        let r = v.readings(a);
        assert_eq!(r.cpu_temp_c, Some(58));
        assert_eq!(r.gpu_temp_c, None, "0 means the dGPU is off");
        assert_eq!(r.max_cooling, Some(false));
        assert_eq!(r.cpu_fan_rpm, Some(5_783)); // 480000 / 0x53
        assert_eq!(r.gpu_fan_rpm, Some(6_075)); // 480000 / 0x4F
        assert_eq!(v.read(a, "msi.fan_mode"), Some(0));
        assert_eq!(v.read(a, "msi.shift_mode"), Some(1));
        assert_eq!(v.read(a, "msi.charge_limit"), Some(60));
    }

    #[test]
    fn cooler_boost_only_touches_its_bit() {
        let ec = FakeEc::stealth();
        let a = access(&ec);
        let v = detect(a).unwrap();
        v.set_max_cooling(a, true).unwrap();
        assert_eq!(ec.mem.borrow()[0x98], 0x82);
        v.set_max_cooling(a, false).unwrap();
        assert_eq!(ec.mem.borrow()[0x98], 0x02);
    }

    #[test]
    fn writes_encode_and_bounds_hold() {
        let ec = FakeEc::stealth();
        let a = access(&ec);
        let v = detect(a).unwrap();
        v.write(a, "msi.fan_mode", 2).unwrap();
        assert_eq!(ec.mem.borrow()[0xD4], 0x8D);
        v.write(a, "msi.charge_limit", 80).unwrap();
        assert_eq!(ec.mem.borrow()[0xD7], 0xD0);
        assert!(v.write(a, "msi.charge_limit", 49).is_err());
        assert!(v.write(a, "msi.fan_mode", 3).is_err());
        assert!(v.write(a, "msi.unknown", 0).is_err());
    }

    #[test]
    fn stopped_fan_reads_zero() {
        assert_eq!(rpm(0), 0);
        assert_eq!(rpm(1), u16::MAX - 1);
    }

    #[test]
    fn unknown_firmware_is_not_detected() {
        let mut ec = FakeEc::stealth();
        ec.fw = "1562EMS1.117";
        assert!(detect(access(&ec)).is_none());
    }

    #[test]
    fn settings_presets_are_valid() {
        for d in SETTINGS {
            d.gaming.map(|g| d.validate(g).unwrap());
            d.validate(d.default.unwrap()).unwrap();
            assert!(d.id.starts_with("msi."));
        }
    }
}
