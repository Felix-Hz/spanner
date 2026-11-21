//! Window and threads.
//! - UI (main): event loop, window and web view; never blocks.
//! - worker: owns `Native` (thread-bound) and answers all requests except history and theme.
//! - history: runs queries; a newer pending query supersedes older ones.

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use tao::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::{Theme as WindowTheme, Window, WindowBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use wry::{NewWindowResponse, Theme as WebTheme, WebContext, WebView, WebViewBuilder};

use crate::assets;
use crate::engine::config::RecorderConfig;
use crate::engine::ipc::{self, Request};
use crate::engine::presets::Presets;
use crate::engine::sampler::Sampler;
use crate::engine::time::{day_of, now_ms};
use crate::engine::ui_state::{Placement, Theme, UiState};
use crate::engine::{history, settings, store};
use crate::error::{Error, Result};
use crate::platform::{Integration, Native, Paths, Platform, System};

pub const ORIGIN: &str = "https://spanner.localhost/";
const TITLE: &str = "spanner";
const DEFAULT_SIZE: LogicalSize<f64> = LogicalSize::new(940.0, 640.0);
const MIN_SIZE: LogicalSize<f64> = LogicalSize::new(760.0, 520.0);

enum UserEvent {
    Reply(String),
    Theme(u32, Theme),
    CloseToTray(u32, bool),
    /// Bring the window back from the tray.
    Show,
    Exit,
}

const MENU_OPEN: &str = "open";
const MENU_QUIT: &str = "quit";

/// Notification-area icon: left click or "Open spanner" shows the window, "Quit spanner" exits.
fn tray(proxy: &EventLoopProxy<UserEvent>) -> Result<TrayIcon> {
    let menu = Menu::new();
    let item = |id: &str, text: &str| MenuItem::with_id(id, text, true, None);
    menu.append(&item(MENU_OPEN, "Open spanner")).map_err(|e| Error(e.to_string()))?;
    menu.append(&PredefinedMenuItem::separator()).map_err(|e| Error(e.to_string()))?;
    menu.append(&item(MENU_QUIT, "Quit spanner")).map_err(|e| Error(e.to_string()))?;

    // Handlers run on other threads; the proxy is shared behind a mutex.
    let clicks = Mutex::new(proxy.clone());
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        let show = matches!(e, TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. })
            || matches!(e, TrayIconEvent::DoubleClick { button: MouseButton::Left, .. });
        if show && let Ok(p) = clicks.lock() {
            let _ = p.send_event(UserEvent::Show);
        }
    }));
    let items = Mutex::new(proxy.clone());
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let event = match e.id.0.as_str() {
            MENU_OPEN => UserEvent::Show,
            MENU_QUIT => UserEvent::Exit,
            _ => return,
        };
        if let Ok(p) = items.lock() {
            let _ = p.send_event(event);
        }
    }));

    let mut builder = TrayIconBuilder::new().with_tooltip(TITLE).with_menu(Box::new(menu)).with_menu_on_left_click(false);
    #[cfg(windows)]
    {
        use tray_icon::Icon;
        if let Ok(icon) = Icon::from_resource(1, Some((32, 32))) {
            builder = builder.with_icon(icon);
        }
    }
    builder.build().map_err(|e| Error(format!("tray icon: {e}")))
}

type Job = (u32, Request);

fn window_theme(t: Theme) -> Option<WindowTheme> {
    match t {
        Theme::System => None,
        Theme::Light => Some(WindowTheme::Light),
        Theme::Dark => Some(WindowTheme::Dark),
    }
}

fn web_theme(t: Theme) -> WebTheme {
    match t {
        Theme::System => WebTheme::Auto,
        Theme::Light => WebTheme::Light,
        Theme::Dark => WebTheme::Dark,
    }
}

/// A saved placement is only reused if its title bar is still on a connected monitor.
fn on_screen(p: &Placement, monitors: impl Iterator<Item = tao::monitor::MonitorHandle>) -> bool {
    let (cx, cy) = (p.x + p.width as i32 / 2, p.y + 16);
    monitors.into_iter().any(|m| {
        let (pos, size) = (m.position(), m.size());
        (pos.x..pos.x + size.width as i32).contains(&cx) && (pos.y..pos.y + size.height as i32).contains(&cy)
    })
}

pub fn run() -> Result<()> {
    // One window: a second launch brings the first forward.
    let Some(_lock) = System::single_instance("spanner_window") else {
        System::activate_existing_window(TITLE);
        return Ok(());
    };

    let state_path = System::ui_state_file()?;
    let mut state = UiState::load(&state_path);

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let mut builder = WindowBuilder::new().with_title(TITLE).with_min_inner_size(MIN_SIZE).with_theme(window_theme(state.theme));
    builder = match state.placement.filter(|p| on_screen(p, event_loop.available_monitors())) {
        Some(p) => builder
            .with_position(PhysicalPosition::new(p.x, p.y))
            .with_inner_size(PhysicalSize::new(p.width, p.height))
            .with_maximized(p.maximized),
        None => builder.with_inner_size(DEFAULT_SIZE),
    };
    #[cfg(windows)]
    {
        use tao::platform::windows::IconExtWindows;
        builder = builder.with_window_icon(tao::window::Icon::from_resource(1, None).ok());
    }
    let window = builder.build(&event_loop).map_err(|e| Error(format!("window: {e}")))?;

    // First frame in the page colour, so the window never flashes white.
    let background = match window.theme() {
        WindowTheme::Dark => (0x0e, 0x11, 0x20, 0xff),
        _ => (0xfa, 0xfa, 0xf8, 0xff),
    };

    let (work_tx, work_rx) = mpsc::channel::<Job>();
    let (hist_tx, hist_rx) = mpsc::channel::<Job>();
    spawn_worker(work_rx, proxy.clone())?;
    spawn_history(hist_rx, proxy.clone())?;

    let mut context = WebContext::new(Some(System::webview_dir()?));
    let ipc_proxy = proxy.clone();
    let mut builder = WebViewBuilder::new_with_web_context(&mut context);
    #[cfg(windows)]
    {
        use wry::WebViewBuilderExtWindows;
        // An app, not a browser: no reload, find, print or zoom keys, and no browser context menu.
        builder = builder
            .with_https_scheme(true)
            .with_browser_accelerator_keys(false)
            .with_default_context_menus(false)
            .with_theme(web_theme(state.theme));
    }
    let webview = builder
        .with_custom_protocol("spanner".into(), |_, req| assets::respond(req.uri().path()))
        .with_url(ORIGIN)
        .with_background_color(background)
        .with_devtools(cfg!(debug_assertions))
        .with_hotkeys_zoom(false)
        .with_navigation_handler(|url| url.starts_with(ORIGIN))
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_ipc_handler(move |req| {
            if !req.uri().to_string().starts_with(ORIGIN) {
                return;
            }
            match ipc::parse(req.body()) {
                Ok((id, Request::SetTheme(t))) => drop(ipc_proxy.send_event(UserEvent::Theme(id, t))),
                Ok((id, Request::SetCloseToTray(on))) => drop(ipc_proxy.send_event(UserEvent::CloseToTray(id, on))),
                Ok((id, r)) => drop(if r.is_history() { hist_tx.send((id, r)) } else { work_tx.send((id, r)) }),
                Err(e) => drop(ipc_proxy.send_event(UserEvent::Reply(ipc::reply_script(0, Err(e))))),
            }
        })
        .build(&window)
        .map_err(|e| Error(format!("webview: {e}")))?;

    // Kept alive for the life of the loop; dropping it removes the icon.
    let _tray = tray(&proxy)?;

    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Reply(script)) => {
                let _ = webview.evaluate_script(&script);
            }
            Event::UserEvent(UserEvent::Theme(id, t)) => {
                set_theme(&window, &webview, t);
                state.theme = t;
                let saved = state.save(&state_path).map(|()| Value::Null);
                let _ = webview.evaluate_script(&ipc::reply_script(id, saved));
            }
            Event::UserEvent(UserEvent::CloseToTray(id, on)) => {
                state.close_to_tray = on;
                let saved = state.save(&state_path).map(|()| Value::Null);
                let _ = webview.evaluate_script(&ipc::reply_script(id, saved));
            }
            Event::UserEvent(UserEvent::Show) => {
                window.set_visible(true);
                window.set_minimized(false);
                window.set_focus();
                let _ = webview.evaluate_script("document.dispatchEvent(new Event('spanner-shown'))");
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } if state.close_to_tray => {
                remember_placement(&window, &mut state);
                let _ = state.save(&state_path);
                window.set_visible(false);
                // The page pauses its polling; a hidden window shouldn't keep reading sensors.
                let _ = webview.evaluate_script("document.dispatchEvent(new Event('spanner-hidden'))");
            }
            Event::UserEvent(UserEvent::Exit) | Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                remember_placement(&window, &mut state);
                let _ = state.save(&state_path);
                *flow = ControlFlow::Exit;
            }
            _ => {}
        }
    })
}

fn set_theme(window: &Window, webview: &WebView, t: Theme) {
    window.set_theme(window_theme(t));
    #[cfg(windows)]
    {
        use wry::WebViewExtWindows;
        let _ = webview.set_theme(web_theme(t));
    }
}

/// Keeps the last normal (non-maximised, non-minimised) bounds so un-maximising restores them.
fn remember_placement(window: &Window, state: &mut UiState) {
    if window.is_minimized() {
        return;
    }
    let maximized = window.is_maximized();
    let previous = state.placement;
    state.placement = match (maximized, previous, window.outer_position()) {
        (true, Some(p), _) => Some(Placement { maximized: true, ..p }),
        (true, None, _) => None,
        (false, _, Ok(pos)) => {
            let size = window.inner_size();
            Some(Placement { x: pos.x, y: pos.y, width: size.width, height: size.height, maximized: false })
        }
        (false, _, Err(_)) => previous,
    };
}

fn reply(proxy: &EventLoopProxy<UserEvent>, id: u32, result: Result<Value>) {
    let _ = proxy.send_event(UserEvent::Reply(ipc::reply_script(id, result)));
}

fn spawn_worker(rx: Receiver<Job>, proxy: EventLoopProxy<UserEvent>) -> Result<()> {
    let body = move || {
        let cfg = System::config_file().map(|p| RecorderConfig::load(&p)).unwrap_or_default();
        let mut sampler = Sampler::<Native>::new(cfg.tracked_set());
        for (id, req) in rx {
            let uninstall = matches!(req, Request::Uninstall { .. });
            let result = handle(&mut sampler, req);
            let ok = result.is_ok();
            reply(&proxy, id, result);
            if uninstall && ok {
                // Give the page a moment to show the outcome before the window closes.
                System::schedule_profile_cleanup();
                thread::sleep(Duration::from_millis(1500));
                let _ = proxy.send_event(UserEvent::Exit);
                return;
            }
        }
    };
    thread::Builder::new().name("worker".into()).spawn(body).map(drop).map_err(Error::from)
}

fn spawn_history(rx: Receiver<Job>, proxy: EventLoopProxy<UserEvent>) -> Result<()> {
    let dir = System::history_dir()?;
    let body = move || {
        while let Ok(mut job) = rx.recv() {
            while let Ok(newer) = rx.try_recv() {
                reply(&proxy, job.0, Err(Error::msg("superseded")));
                job = newer;
            }
            let result = match job.1 {
                Request::History(q) => {
                    let tracked = System::config_file().map(|p| RecorderConfig::load(&p).tracked_apps).unwrap_or_default();
                    history::run(&dir, &tracked, &q).and_then(|r| Ok(serde_json::to_value(r)?))
                }
                _ => Err(Error::msg("not a history request")),
            };
            reply(&proxy, job.0, result);
        }
    };
    thread::Builder::new().name("history".into()).spawn(body).map(drop).map_err(Error::from)
}

fn last_sample_ms() -> Option<i64> {
    store::latest(&System::history_dir().ok()?, day_of(now_ms())).ok().flatten().map(|r| r.ts_ms)
}

fn handle(s: &mut Sampler<Native>, req: Request) -> Result<Value> {
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
                "recorder_running": System::recorder_running(),
            })
        }
        Request::Settings => {
            let mut views = settings::views(&s.platform, s.vendor(), &System::config_file()?);
            // "When hot" is carried out by the recorder; say so when it isn't running.
            if let Some(v) = views.iter_mut().find(|v| v.id == "full_speed" && v.current == Some(settings::FULL_SPEED_WHEN_HOT))
                && !System::recorder_running()
            {
                v.status = Some("Needs background recording, which is off. Turn it on in Settings.".into());
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
            "running": System::recorder_running(),
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
            match sw.running {
                Some(true) => System::start_recorder()?,
                Some(false) => System::stop_recorder()?,
                None => {}
            }
            Value::Null
        }
        Request::InstallRecorder => {
            System::install()?;
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
        Request::Presets => serde_json::to_value(Presets::load(&System::presets_file()?))?,
        Request::SavePreset(p) => serde_json::to_value(Presets::upsert(&System::presets_file()?, p)?)?,
        Request::DeletePreset(name) => serde_json::to_value(Presets::delete(&System::presets_file()?, &name)?)?,
        Request::History(_) | Request::SetTheme(_) | Request::SetCloseToTray(_) => return Err(Error::msg("handled on another thread")),
    })
}
