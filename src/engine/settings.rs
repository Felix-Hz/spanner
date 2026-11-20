//! Settings as data. Generic power settings are defined here; vendor drivers add their own `Def`s.
//! The UI is generated from the combined list, and every incoming value is validated against its
//! `Def` before anything is written.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::config::RecorderConfig;
use super::fan_policy::{CPU_OFF_C, CPU_ON_C, GPU_OFF_C, GPU_ON_C};
use crate::error::{Error, Result};
use crate::hw::{Access, Vendor};
use crate::platform::{Knob, Platform};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Performance,
    Cooling,
    Battery,
}

#[derive(Debug, Serialize)]
pub struct Opt {
    pub value: i64,
    pub label: &'static str,
    /// One line shown under the control when this option is selected.
    pub help: &'static str,
}

pub enum Kind {
    /// Rendered as a slider. `step` only affects the slider; any whole value in range is valid.
    /// `ends` labels the low and high ends when the number alone means little.
    Number { min: i64, max: i64, step: i64, unit: &'static str, ends: Option<(&'static str, &'static str)> },
    /// Two options with values 0 and 1 render as a switch; up to four as a segmented control.
    Choice(&'static [Opt]),
}

#[derive(Clone, Copy)]
pub enum Target {
    /// OS power management, through `Platform`.
    Knob(Knob),
    /// Handled by the active vendor driver, keyed by `Def::id`.
    Vendor,
    /// Off, Always (vendor max cooling) or When hot (recorder policy). Shown only if the vendor supports it.
    FullSpeed,
}

pub struct Def {
    pub id: &'static str,
    pub section: Section,
    /// Shown under "Advanced" in its section.
    pub advanced: bool,
    pub name: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    pub target: Target,
    /// `None`: use the OS shipped default (mains value).
    pub default: Option<i64>,
    /// Value used by the gaming preset; `None` leaves the setting alone.
    pub gaming: Option<i64>,
}

impl Def {
    pub fn validate(&self, v: i64) -> Result<i64> {
        let ok = match self.kind {
            Kind::Number { min, max, .. } => (min..=max).contains(&v),
            Kind::Choice(opts) => opts.iter().any(|o| o.value == v),
        };
        ok.then_some(v).ok_or_else(|| Error(format!("{v} isn't a valid value for {}.", self.name)))
    }
}

/// Values 0 and 1, rendered as a switch.
pub const OFF_ON: &[Opt] = &[Opt { value: 0, label: "Off", help: "" }, Opt { value: 1, label: "On", help: "" }];

/// Full-speed fan modes.
pub const FULL_SPEED_OFF: i64 = 0;
pub const FULL_SPEED_ALWAYS: i64 = 1;
pub const FULL_SPEED_WHEN_HOT: i64 = 2;

const FULL_SPEED_MODES: &[Opt] = &[
    Opt { value: FULL_SPEED_OFF, label: "Off", help: "The fan profile sets the speed." },
    Opt { value: FULL_SPEED_WHEN_HOT, label: "When hot", help: "" },
    Opt { value: FULL_SPEED_ALWAYS, label: "Always", help: "Both fans at maximum all the time, whatever the temperature." },
];

const BOOST_MODES: &[Opt] = &[
    Opt { value: 0, label: "Off", help: "The CPU never boosts." },
    Opt { value: 3, label: "Efficient", help: "Boosts only when it saves time without costing much power." },
    Opt { value: 1, label: "Standard", help: "Boosts when the load calls for it." },
    Opt { value: 4, label: "Efficient aggressive", help: "Boosts readily, with some regard for power." },
    Opt { value: 2, label: "Aggressive", help: "Boosts whenever it can." },
    Opt { value: 6, label: "Efficient aggressive, guaranteed", help: "As Efficient aggressive, holding at least base speed." },
    Opt { value: 5, label: "Aggressive, guaranteed", help: "As Aggressive, holding at least base speed." },
];

/// Settings every machine has.
pub const GENERIC: &[Def] = &[
    Def {
        id: "proc_max",
        section: Section::Performance,
        advanced: true,
        name: "Maximum processor state",
        help: "Caps CPU speed. Anything below 100% also turns off turbo boost.",
        kind: Kind::Number { min: 5, max: 100, step: 1, unit: "%", ends: None },
        target: Target::Knob(Knob::ProcessorMax),
        default: None,
        gaming: Some(99),
    },
    Def {
        id: "proc_min",
        section: Section::Performance,
        advanced: true,
        name: "Minimum processor state",
        help: "The lowest speed the CPU drops to when idle.",
        kind: Kind::Number { min: 0, max: 100, step: 1, unit: "%", ends: None },
        target: Target::Knob(Knob::ProcessorMin),
        default: None,
        gaming: None,
    },
    Def {
        id: "boost_mode",
        section: Section::Performance,
        advanced: true,
        name: "Turbo boost mode",
        help: "How readily the CPU boosts. Only applies while turbo boost is on.",
        kind: Kind::Choice(BOOST_MODES),
        target: Target::Knob(Knob::BoostMode),
        default: None,
        gaming: None,
    },
    Def {
        id: "epp",
        section: Section::Performance,
        advanced: true,
        name: "Energy preference",
        help: "Where the CPU sits between speed and battery life when it picks its own clock.",
        kind: Kind::Number { min: 0, max: 100, step: 5, unit: "", ends: Some(("Performance", "Battery life")) },
        target: Target::Knob(Knob::EnergyPreference),
        default: None,
        gaming: Some(25),
    },
    Def {
        id: "full_speed",
        section: Section::Cooling,
        advanced: false,
        name: "Full-speed fans",
        help: "Runs both fans at maximum, overriding the fan profile.",
        kind: Kind::Choice(FULL_SPEED_MODES),
        target: Target::FullSpeed,
        default: Some(FULL_SPEED_OFF),
        gaming: Some(FULL_SPEED_WHEN_HOT),
    },
    Def {
        id: "cooling_policy",
        section: Section::Cooling,
        advanced: false,
        name: "Cool before slowing down",
        help: "When the system heats up, raise fan speed first and reduce CPU speed only if that isn't enough.",
        kind: Kind::Choice(OFF_ON),
        target: Target::Knob(Knob::CoolingPolicy),
        default: None,
        gaming: Some(1),
    },
];

/// Generic settings first, then the vendor's. Full-speed fans only if the vendor can do it.
fn all(vendor: Option<&dyn Vendor>) -> impl Iterator<Item = &Def> {
    let max_cooling = vendor.is_some_and(|v| v.supports_max_cooling());
    GENERIC
        .iter()
        .filter(move |d| max_cooling || !matches!(d.target, Target::FullSpeed))
        .chain(vendor.map(|v| v.settings()).unwrap_or_default())
}

fn find<'v>(vendor: Option<&'v dyn Vendor>, id: &str) -> Option<&'v Def> {
    all(vendor).find(|d| d.id == id)
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Control {
    Number { min: i64, max: i64, step: i64, unit: &'static str, ends: Option<(&'static str, &'static str)> },
    Choice { options: &'static [Opt] },
}

#[derive(Serialize)]
pub struct View {
    pub id: &'static str,
    pub section: Section,
    pub advanced: bool,
    pub name: &'static str,
    pub help: &'static str,
    pub control: Control,
    /// Mains value.
    pub current: Option<i64>,
    /// Set only when the battery value differs.
    pub current_dc: Option<i64>,
    pub default: Option<i64>,
    pub gaming: Option<i64>,
    /// Whether it is in effect right now, where that can differ from its value ("When hot").
    pub active: Option<bool>,
    /// Current state for a line under the control.
    pub status: Option<String>,
    /// Why the setting can't be shown or changed, if so.
    pub unavailable: Option<String>,
}

pub fn views<P: Platform>(p: &P, vendor: Option<&dyn Vendor>, config: &Path) -> Vec<View> {
    let access = Access { ec: p.ec() };
    let readings = vendor.map(|v| v.readings(access)).unwrap_or_default();
    let auto = RecorderConfig::load(config).auto_full_speed;
    all(vendor)
        .map(|d| {
            let control = match d.kind {
                Kind::Number { min, max, step, unit, ends } => Control::Number { min, max, step, unit, ends },
                Kind::Choice(options) => Control::Choice { options },
            };
            let (mut active, mut status) = (None, None);
            let (current, current_dc, default, unavailable) = match d.target {
                Target::Knob(k) => match p.knob(k) {
                    Ok(v) => {
                        let def = d.default.or_else(|| p.knob_default(k).ok().map(|v| i64::from(v.ac)));
                        (Some(i64::from(v.ac)), (v.dc != v.ac).then(|| i64::from(v.dc)), def, None)
                    }
                    Err(e) => (None, None, d.default, Some(format!("Windows didn't return this setting ({e})."))),
                },
                Target::Vendor => {
                    let cur = vendor.and_then(|v| v.read(access, d.id));
                    let why = cur.is_none().then(|| "The firmware didn't return a value.".to_string());
                    (cur, None, d.default, why)
                }
                Target::FullSpeed => {
                    active = readings.max_cooling;
                    let cur = match (auto, readings.max_cooling) {
                        (true, _) => Some(FULL_SPEED_WHEN_HOT),
                        (false, Some(on)) => Some(if on { FULL_SPEED_ALWAYS } else { FULL_SPEED_OFF }),
                        (false, None) => None,
                    };
                    if auto {
                        status = Some(match readings.max_cooling {
                            Some(true) => {
                                format!("On now. Turns off once the CPU is below {CPU_OFF_C} °C and the GPU below {GPU_OFF_C} °C.")
                            }
                            _ => format!("Off now. Turns on when the CPU reaches {CPU_ON_C} °C or the GPU {GPU_ON_C} °C."),
                        });
                    }
                    let why = cur.is_none().then(|| "The firmware didn't return a value.".to_string());
                    (cur, None, d.default, why)
                }
            };
            View {
                id: d.id,
                section: d.section,
                advanced: d.advanced,
                name: d.name,
                help: d.help,
                control,
                current,
                current_dc,
                default,
                gaming: d.gaming,
                active,
                status,
                unavailable,
            }
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub id: String,
    pub value: i64,
}

#[derive(Serialize)]
pub struct Outcome {
    pub id: String,
    pub ok: bool,
    pub error: Option<String>,
}

const MAX_CHANGES: usize = 32;

/// Every change is validated before the first write; one invalid value means nothing is written.
pub fn apply<P: Platform>(p: &P, vendor: Option<&dyn Vendor>, config: &Path, changes: &[Change]) -> Result<Vec<Outcome>> {
    if changes.len() > MAX_CHANGES {
        return Err(Error::msg("Too many changes at once."));
    }
    let plan = changes
        .iter()
        .map(|c| {
            let d = find(vendor, &c.id).ok_or_else(|| Error(format!("Unknown setting '{}'.", c.id)))?;
            Ok((d, d.validate(c.value)?))
        })
        .collect::<Result<Vec<_>>>()?;

    let access = Access { ec: p.ec() };
    Ok(plan
        .into_iter()
        .map(|(d, v)| {
            let res = match d.target {
                Target::Knob(k) => p.set_knob(k, v as u32),
                Target::Vendor => vendor.ok_or_else(|| Error::msg("No vendor driver.")).and_then(|vd| vd.write(access, d.id, v)),
                Target::FullSpeed => set_full_speed(vendor, access, config, v),
            };
            Outcome { id: d.id.to_string(), ok: res.is_ok(), error: res.err().map(|e| e.0) }
        })
        .collect())
}

/// Off and Always switch the fans now; When hot hands control to the recorder.
fn set_full_speed(vendor: Option<&dyn Vendor>, access: Access, config: &Path, mode: i64) -> Result<()> {
    let vendor = vendor.ok_or_else(|| Error::msg("No vendor driver."))?;
    let mut cfg = RecorderConfig::load(config);
    let auto = mode == FULL_SPEED_WHEN_HOT;
    if cfg.auto_full_speed != auto {
        cfg.auto_full_speed = auto;
        cfg.save(config)?;
    }
    if auto { Ok(()) } else { vendor.set_max_cooling(access, mode == FULL_SPEED_ALWAYS) }
}

pub fn defaults_as_changes<P: Platform>(p: &P, vendor: Option<&dyn Vendor>, config: &Path) -> Vec<Change> {
    views(p, vendor, config)
        .into_iter()
        .filter(|v| v.unavailable.is_none())
        .filter_map(|v| Some(Change { id: v.id.into(), value: v.default? }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_ids_unique_and_presets_valid() {
        for (i, d) in GENERIC.iter().enumerate() {
            assert!(GENERIC[i + 1..].iter().all(|o| o.id != d.id), "{}", d.id);
            assert!(!d.id.contains('.'), "generic ids are unprefixed; vendor ids use '<vendor>.'");
            if let Some(g) = d.gaming {
                d.validate(g).unwrap();
            }
        }
    }

    #[test]
    fn validation_bounds() {
        let max = find(None, "proc_max").unwrap();
        assert!(max.validate(101).is_err());
        assert!(max.validate(4).is_err());
        assert_eq!(max.validate(99).unwrap(), 99);
        assert!(find(None, "boost_mode").unwrap().validate(7).is_err());
        assert!(find(None, "msi.fan_mode").is_none(), "vendor settings need a driver");
    }

    #[test]
    fn boost_mode_lists_every_windows_value_once() {
        let Kind::Choice(opts) = find(None, "boost_mode").unwrap().kind else { panic!() };
        let mut v: Vec<i64> = opts.iter().map(|o| o.value).collect();
        v.sort_unstable();
        assert_eq!(v, (0..=6).collect::<Vec<_>>());
    }
}
