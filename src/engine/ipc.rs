//! The message protocol between the page and the backend. The page sends `{id, method, params}`; every
//! method and its parameters are parsed into typed requests here, and anything else is rejected.

use serde::Deserialize;
use serde_json::{Value, json};

use super::config::RecorderConfig;
use super::history::Query;
use super::presets::Preset;
use super::settings::Change;
use super::ui_state::Theme;
use crate::error::{Error, Result};

/// Messages are small; anything bigger is not from our page.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    id: u32,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyParams {
    changes: Vec<Change>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UninstallParams {
    restore_defaults: bool,
}

/// Either or both; omitted fields are left as they are.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecorderSwitches {
    pub running: Option<bool>,
    pub start_at_sign_in: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameParams {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OnParams {
    on: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeParams {
    theme: Theme,
}

pub enum Request {
    Overview,
    /// Cheap periodic state for the status bar.
    Status,
    Settings,
    Apply(Vec<Change>),
    Live,
    /// Hardware and OS description; read once per run.
    SystemInfo,
    History(Query),
    RecorderStatus,
    SaveRecorder(RecorderConfig),
    InstallRecorder,
    SetRecorder(RecorderSwitches),
    Uninstall {
        restore_defaults: bool,
    },
    Presets,
    SavePreset(Preset),
    DeletePreset(String),
    /// Handled on the UI thread: it owns the window.
    SetTheme(Theme),
    /// Handled on the UI thread.
    SetCloseToTray(bool),
}

impl Request {
    /// History queries run on their own thread so a long range never delays the live view.
    pub fn is_history(&self) -> bool {
        matches!(self, Self::History(_))
    }
}

pub fn parse(body: &str) -> Result<(u32, Request)> {
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(Error::msg("message too large"));
    }
    let env: Envelope = serde_json::from_str(body)?;
    let p = env.params;
    let req = match env.method.as_str() {
        "overview" => Request::Overview,
        "status" => Request::Status,
        "settings" => Request::Settings,
        "apply" => Request::Apply(serde_json::from_value::<ApplyParams>(p)?.changes),
        "live" => Request::Live,
        "system_info" => Request::SystemInfo,
        "history" => Request::History(serde_json::from_value(p)?),
        "recorder_status" => Request::RecorderStatus,
        "save_recorder" => Request::SaveRecorder(serde_json::from_value(p)?),
        "install_recorder" => Request::InstallRecorder,
        "set_recorder" => Request::SetRecorder(serde_json::from_value(p)?),
        "uninstall" => Request::Uninstall { restore_defaults: serde_json::from_value::<UninstallParams>(p)?.restore_defaults },
        "presets" => Request::Presets,
        "save_preset" => Request::SavePreset(serde_json::from_value(p)?),
        "delete_preset" => Request::DeletePreset(serde_json::from_value::<NameParams>(p)?.name),
        "set_close_to_tray" => Request::SetCloseToTray(serde_json::from_value::<OnParams>(p)?.on),
        "set_theme" => Request::SetTheme(serde_json::from_value::<ThemeParams>(p)?.theme),
        other => return Err(Error(format!("unknown method '{other}'"))),
    };
    Ok((env.id, req))
}

/// The script that resolves the page's pending promise. serde_json output is a valid JS expression.
pub fn reply_script(id: u32, result: Result<Value>) -> String {
    let body = match result {
        Ok(data) => json!({ "ok": true, "data": data }),
        Err(e) => json!({ "ok": false, "error": e.0 }),
    };
    format!("window.__spannerReply({id},{body})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_methods_and_rejects_the_rest() {
        assert!(matches!(parse(r#"{"id":1,"method":"live"}"#).unwrap().1, Request::Live));
        let (_, r) = parse(r#"{"id":2,"method":"apply","params":{"changes":[{"id":"proc_max","value":99}]}}"#).unwrap();
        assert!(matches!(r, Request::Apply(c) if c.len() == 1 && c[0].value == 99));
        assert!(parse(r#"{"id":3,"method":"eval"}"#).is_err());
        assert!(matches!(parse(r#"{"id":5,"method":"set_theme","params":{"theme":"dark"}}"#).unwrap().1, Request::SetTheme(Theme::Dark)));
        assert!(parse(r#"{"id":6,"method":"set_theme","params":{"theme":"pink"}}"#).is_err());
        assert!(parse(r#"{"id":4,"method":"apply","params":{"changes":[],"extra":1}}"#).is_err());
        assert!(parse(&"x".repeat(MAX_MESSAGE_BYTES + 1)).is_err());
    }

    #[test]
    fn reply_is_a_call_with_json() {
        assert_eq!(reply_script(7, Ok(json!([1]))), r#"window.__spannerReply(7,{"data":[1],"ok":true})"#);
    }
}
