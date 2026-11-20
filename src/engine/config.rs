//! Recorder settings, stored as JSON next to the history. The recorder re-reads the file when it changes.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A program whose samples are tagged, so History can show only the time it was running.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "TrackedAppRepr")]
pub struct TrackedApp {
    /// Executable file name, e.g. `PlanetZoo.exe`.
    pub exe: String,
    /// Display name, e.g. `Planet Zoo`. Apps sharing a name are shown as one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Accepts the older plain-string form as well as `{exe, name}`.
#[derive(Deserialize)]
#[serde(untagged)]
enum TrackedAppRepr {
    Exe(String),
    Full {
        exe: String,
        #[serde(default)]
        name: Option<String>,
    },
}

impl From<TrackedAppRepr> for TrackedApp {
    fn from(r: TrackedAppRepr) -> Self {
        match r {
            TrackedAppRepr::Exe(exe) => Self { exe, name: None },
            TrackedAppRepr::Full { exe, name } => Self { exe, name: name.filter(|n| !n.trim().is_empty()) },
        }
    }
}

impl TrackedApp {
    pub fn new(exe: &str, name: &str) -> Self {
        Self { exe: exe.into(), name: Some(name.into()) }
    }

    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.exe)
    }
}

/// Stable 32-bit id stored with each sample: FNV-1a of the lower-cased file name. 0 means "none".
pub fn app_id(exe: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in exe.to_lowercase().bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h.max(1)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecorderConfig {
    pub interval_secs: u32,
    pub retention_days: u32,
    pub tracked_apps: Vec<TrackedApp>,
    /// Full-speed fans "When hot": the recorder switches them by temperature (see `fan_policy`).
    #[serde(default)]
    pub auto_full_speed: bool,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            interval_secs: 5,
            retention_days: 30,
            tracked_apps: vec![TrackedApp::new("PlanetZoo.exe", "Planet Zoo")],
            auto_full_speed: false,
        }
    }
}

pub const INTERVAL_RANGE: std::ops::RangeInclusive<u32> = 2..=60;
pub const RETENTION_RANGE: std::ops::RangeInclusive<u32> = 1..=365;
const MAX_APPS: usize = 32;
const MAX_LABEL: usize = 40;

fn valid_exe_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (5..=64).contains(&name.len()) && lower.ends_with(".exe") && name.chars().all(|c| c.is_ascii_alphanumeric() || " ._-()+".contains(c))
}

impl RecorderConfig {
    pub fn validate(&self) -> Result<()> {
        if !INTERVAL_RANGE.contains(&self.interval_secs) {
            return Err(Error::msg("The sample interval has to be between 2 and 60 seconds."));
        }
        if !RETENTION_RANGE.contains(&self.retention_days) {
            return Err(Error::msg("History can be kept for 1 to 365 days."));
        }
        if self.tracked_apps.len() > MAX_APPS {
            return Err(Error(format!("You can track up to {MAX_APPS} apps.")));
        }
        if let Some(bad) = self.tracked_apps.iter().find(|a| !valid_exe_name(&a.exe)) {
            return Err(Error(format!("\"{}\" isn't a program file name. Use the name of the .exe, for example Game.exe.", bad.exe)));
        }
        if self
            .tracked_apps
            .iter()
            .filter_map(|a| a.name.as_deref())
            .any(|n| n.chars().count() > MAX_LABEL || n.chars().any(char::is_control))
        {
            return Err(Error(format!("App names can be up to {MAX_LABEL} characters.")));
        }
        Ok(())
    }

    /// Lower-cased executable names, for matching running processes.
    pub fn tracked_set(&self) -> HashSet<String> {
        self.tracked_apps.iter().map(|a| a.exe.to_lowercase()).collect()
    }

    /// Falls back to defaults if the file is missing or invalid, so a bad edit never stops recording.
    pub fn load(path: &Path) -> Self {
        fs::read(path).ok().and_then(|b| serde_json::from_slice::<Self>(&b).ok()).filter(|c| c.validate().is_ok()).unwrap_or_default()
    }

    /// Writes via a temporary file and rename, so the recorder never reads a half-written file.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid() {
        RecorderConfig::default().validate().unwrap();
    }

    #[test]
    fn rejects_paths_and_odd_names() {
        for bad in ["..\\evil.exe", "C:\\x.exe", "game", "a.bat", "x\".exe"] {
            let c = RecorderConfig { tracked_apps: vec![TrackedApp { exe: bad.into(), name: None }], ..Default::default() };
            assert!(c.validate().is_err(), "{bad}");
        }
        let ok = RecorderConfig { tracked_apps: vec![TrackedApp::new("Planet Zoo (x64).exe", "Planet Zoo")], ..Default::default() };
        ok.validate().unwrap();
    }

    #[test]
    fn reads_old_string_form_and_new_object_form() {
        let json = r#"{"interval_secs":5,"retention_days":30,"tracked_apps":["sekiro.exe",{"exe":"PlanetZoo.exe","name":"Planet Zoo"}]}"#;
        let c: RecorderConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.tracked_apps[0], TrackedApp { exe: "sekiro.exe".into(), name: None });
        assert_eq!(c.tracked_apps[1].label(), "Planet Zoo");
        assert!(!c.auto_full_speed);
    }

    #[test]
    fn app_id_is_case_insensitive_and_never_zero() {
        assert_eq!(app_id("PlanetZoo.exe"), app_id("planetzoo.EXE"));
        assert_ne!(app_id("PlanetZoo.exe"), app_id("sekiro.exe"));
        assert_ne!(app_id(""), 0);
    }

    #[test]
    fn load_falls_back_on_garbage() {
        let p = std::env::temp_dir().join(format!("spanner_cfg_{}.json", std::process::id()));
        fs::write(&p, b"{not json").unwrap();
        assert_eq!(RecorderConfig::load(&p), RecorderConfig::default());
        let c = RecorderConfig { interval_secs: 10, ..Default::default() };
        c.save(&p).unwrap();
        assert_eq!(RecorderConfig::load(&p), c);
        fs::remove_file(p).unwrap();
    }
}
