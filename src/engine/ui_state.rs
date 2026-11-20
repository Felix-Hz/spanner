//! Per-user window state: theme and last window placement. Lost or invalid state falls back to defaults.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// Outer position and inner size in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub theme: Theme,
    pub placement: Option<Placement>,
}

impl UiState {
    pub fn load(path: &Path) -> Self {
        fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Temp file and rename, so a crash mid-write can't leave a truncated file.
    pub fn save(&self, path: &Path) -> Result<()> {
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
    fn round_trips_and_tolerates_garbage() {
        let p = std::env::temp_dir().join(format!("spanner_ui_{}.json", std::process::id()));
        fs::write(&p, b"nope").unwrap();
        assert_eq!(UiState::load(&p), UiState::default());
        let s = UiState { theme: Theme::Dark, placement: Some(Placement { x: -8, y: 10, width: 940, height: 640, maximized: false }) };
        s.save(&p).unwrap();
        assert_eq!(UiState::load(&p), s);
        fs::remove_file(p).unwrap();
    }
}
