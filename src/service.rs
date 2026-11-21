//! Requests that touch hardware, power plans or the shared data folder. They run in the recorder, which
//! is elevated through Task Scheduler, and reach it from the unelevated window over the broker pipe.
//!
//! The broker serves only `brokered` requests and validates each the same way the window would, so a
//! process of the same user gains nothing beyond spanner's own settings by talking to it.

use serde_json::{Value, json};

use crate::engine::config::RecorderConfig;
use crate::engine::ipc::{self, Request};
use crate::engine::sampler::Sampler;
use crate::engine::time::{day_of, now_ms};
use crate::engine::ui_state::UiState;
use crate::engine::{history, settings, store};
use crate::error::{Error, Result};
use crate::platform::{Integration, Native, Paths, Platform, System};

pub fn last_sample_ms() -> Option<i64> {
    store::latest(&System::history_dir().ok()?, day_of(now_ms())).ok().flatten().map(|r| r.ts_ms)
}

/// Recording is on when the recorder runs and its config asks it to write samples.
pub fn recording() -> bool {
    System::recorder_running() && System::config_file().map(|p| RecorderConfig::load(&p).record).unwrap_or(false)
}

/// What the broker serves. Presets and history are the user's own files and stay in the window; theme
/// and close-to-tray belong to the window; installing needs its own consent prompt.
pub fn brokered(req: &Request) -> bool {
    !matches!(
        req,
        Request::Presets
            | Request::SavePreset(_)
            | Request::DeletePreset(_)
            | Request::History(_)
            | Request::SetTheme(_)
            | Request::SetCloseToTray(_)
            | Request::InstallRecorder
    )
}

/// One broker exchange: the window's request envelope in, `{"ok": true, "data": ..}` or `{"ok": false,
/// "error": ..}` out.
pub fn broker_reply(s: &mut Sampler<Native>, body: &str) -> String {
    let result = ipc::parse(body).and_then(|(_, req)| {
        if brokered(&req) { handle(s, req) } else { Err(Error::msg("not a request the background service handles")) }
    });
    match result {
        Ok(data) => json!({ "ok": true, "data": data }),
        Err(e) => json!({ "ok": false, "error": e.0 }),
    }
    .to_string()
}

pub fn handle(s: &mut Sampler<Native>, req: Request) -> Result<Value> {
    Ok(match req {
        Request::Overview => json!({
            "version": env!("CARGO_PKG_VERSION"),
            "machine": s.vendor().map(|v| v.describe()),
            "theme": System::ui_state_file().map(|p| UiState::load(&p).theme).unwrap_or_default(),
            "close_to_tray": System::ui_state_file().map(|p| UiState::load(&p).close_to_tray).unwrap_or(true),
            "recorder_installed": System::recorder_installed(),
            "metrics": history::METRICS.iter().map(|m| json!({ "id": m.id, "label": m.label, "unit": m.unit, "group": m.group })).collect::<Vec<_>>(),
        }),
        Request::Status => {
            let power = s.platform.power_status();
            json!({
                "on_ac": power.on_ac,
                "battery_pct": power.battery_pct,
                "power_profile": s.platform.power_profile_name(),
                "last_sample_ms": last_sample_ms(),
                "recorder_running": recording(),
            })
        }
        Request::Settings => {
            let mut views = settings::views(&s.platform, s.vendor(), &System::config_file()?);
            // "When hot" is carried out by the recorder; say so when it isn't running.
            if let Some(v) = views.iter_mut().find(|v| v.id == "full_speed" && v.current == Some(settings::FULL_SPEED_WHEN_HOT))
                && !System::recorder_running()
            {
                v.status = Some("Needs the background service, which isn't running. Restart spanner to start it.".into());
                v.active = Some(false);
            }
            serde_json::to_value(views)?
        }
        Request::Apply(changes) => serde_json::to_value(settings::apply(&s.platform, s.vendor(), &System::config_file()?, &changes)?)?,
        Request::Live => serde_json::to_value(s.live())?,
        Request::SystemInfo => {
            // The hardware doesn't change while spanner runs, and the WMI reads take a moment.
            static INFO: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
            INFO.get_or_init(|| serde_json::to_value(s.system_info()).unwrap_or(Value::Null)).clone()
        }
        Request::RecorderStatus => json!({
            "installed": System::recorder_installed(),
            "running": recording(),
            "start_at_sign_in": System::starts_at_sign_in(),
            "last_sample_ms": last_sample_ms(),
            "config": RecorderConfig::load(&System::config_file()?),
        }),
        Request::SaveRecorder(cfg) => {
            cfg.save(&System::config_file()?)?;
            s.tracked = cfg.tracked_set();
            Value::Null
        }
        Request::SetRecorder(sw) => {
            if let Some(on) = sw.start_at_sign_in {
                System::set_start_at_sign_in(on)?;
            }
            // Recording on or off is a config flag: the recorder keeps running as the broker either way.
            if let Some(on) = sw.running {
                let path = System::config_file()?;
                let mut cfg = RecorderConfig::load(&path);
                cfg.record = on;
                cfg.save(&path)?;
                if on {
                    System::start_recorder()?;
                }
            }
            Value::Null
        }
        Request::Uninstall { restore_defaults } => {
            let mut problems = Vec::new();
            if restore_defaults {
                let config = System::config_file()?;
                let defaults = settings::defaults_as_changes(&s.platform, s.vendor(), &config);
                let outcomes = settings::apply(&s.platform, s.vendor(), &config, &defaults)?;
                problems.extend(outcomes.into_iter().filter_map(|o| Some(format!("{}: {}", o.id, o.error?))));
            }
            problems.extend(System::uninstall()?);
            json!({ "problems": problems })
        }
        Request::Presets
        | Request::SavePreset(_)
        | Request::DeletePreset(_)
        | Request::History(_)
        | Request::SetTheme(_)
        | Request::SetCloseToTray(_)
        | Request::InstallRecorder => return Err(Error::msg("handled in the window")),
    })
}

#[cfg(test)]
mod tests {
    use crate::engine::ipc::Request;

    #[test]
    fn broker_refuses_window_only_requests() {
        assert!(!super::brokered(&Request::InstallRecorder));
        assert!(!super::brokered(&Request::Presets));
        assert!(super::brokered(&Request::Live));
        assert!(super::brokered(&Request::Settings));
    }
}
