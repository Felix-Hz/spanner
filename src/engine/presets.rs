//! User presets: named snapshots of setting values, stored per user. Values are not validated here;
//! applying a preset goes through `settings::apply`, which validates every value first.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const MAX_PRESETS: usize = 30;
const MAX_NAME: usize = 40;
const MAX_VALUES: usize = 64;
const MAX_ID: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub name: String,
    /// Setting id to value. Sorted, so the file diffs cleanly.
    pub values: BTreeMap<String, i64>,
}

impl Preset {
    fn validate(&self) -> Result<()> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME {
            return Err(Error(format!("A preset name needs 1 to {MAX_NAME} characters.")));
        }
        if name.chars().any(char::is_control) {
            return Err(Error::msg("A preset name can't contain control characters."));
        }
        if self.values.is_empty() || self.values.len() > MAX_VALUES {
            return Err(Error::msg("A preset needs at least one setting."));
        }
        if self
            .values
            .keys()
            .any(|id| id.is_empty() || id.len() > MAX_ID || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        {
            return Err(Error::msg("A preset contains an invalid setting id."));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presets(pub Vec<Preset>);

impl Presets {
    /// Missing or unreadable file: no presets.
    pub fn load(path: &Path) -> Self {
        fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    fn position(&self, name: &str) -> Option<usize> {
        self.0.iter().position(|p| p.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Adds the preset, or replaces one with the same name (case-insensitive).
    pub fn upsert(path: &Path, mut preset: Preset) -> Result<Self> {
        preset.validate()?;
        preset.name = preset.name.trim().to_string();
        let mut all = Self::load(path);
        match all.position(&preset.name) {
            Some(i) => all.0[i] = preset,
            None if all.0.len() >= MAX_PRESETS => {
                return Err(Error(format!("You can keep up to {MAX_PRESETS} presets. Delete one first.")));
            }
            None => all.0.push(preset),
        }
        all.save(path)?;
        Ok(all)
    }

    pub fn delete(path: &Path, name: &str) -> Result<Self> {
        let mut all = Self::load(path);
        let i = all.position(name).ok_or_else(|| Error(format!("There's no preset called \"{name}\".")))?;
        all.0.remove(i);
        all.save(path)?;
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(name: &str) -> Preset {
        Preset { name: name.into(), values: BTreeMap::from([("proc_max".into(), 99), ("msi.fan_mode".into(), 2)]) }
    }

    #[test]
    fn upsert_replaces_by_name_and_delete_removes() {
        let p = std::env::temp_dir().join(format!("spanner_presets_{}.json", std::process::id()));
        let _ = fs::remove_file(&p);
        Presets::upsert(&p, preset("Quiet")).unwrap();
        let mut louder = preset("  quiet ");
        louder.values.insert("full_speed".into(), 1);
        let all = Presets::upsert(&p, louder).unwrap();
        assert_eq!(all.0.len(), 1, "same name, any case, replaces");
        assert_eq!(all.0[0].name, "quiet");
        assert_eq!(all.0[0].values.len(), 3);
        assert!(Presets::delete(&p, "QUIET").unwrap().0.is_empty());
        assert!(Presets::delete(&p, "missing").is_err());
        fs::remove_file(p).unwrap();
    }

    #[test]
    fn rejects_bad_presets() {
        assert!(preset("").validate().is_err());
        assert!(preset(&"x".repeat(41)).validate().is_err());
        assert!(preset("a\u{7}b").validate().is_err());
        let mut bad = preset("ok");
        bad.values.insert("../x".into(), 1);
        assert!(bad.validate().is_err());
        assert!(Preset { name: "ok".into(), values: BTreeMap::new() }.validate().is_err());
    }
}
