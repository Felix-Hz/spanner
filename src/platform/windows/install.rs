//! Task Scheduler entry for the recorder, Installed apps entry, Desktop shortcut.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, IPersistFile};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegSetValueExW,
};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW, CreateEventW, CreateMutexW, GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenMutexW,
    OpenProcessToken, ResetEvent, SYNCHRONIZATION_SYNCHRONIZE, SetEvent, SetPriorityClass, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{IShellLinkW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, Interface, PCWSTR, w};

use super::com::Com;
use super::paths;
use crate::engine::config::RecorderConfig;
use crate::error::{Error, Result, win32};
use crate::platform::{InstanceGuard, Integration, Paths};

const TASK_NAME: &str = r"\spanner\recorder";
pub const RECORDER_MUTEX: &str = r"Global\spanner_recorder";
const RECORDER_STOP_EVENT: &str = r"Global\spanner_recorder_stop";

/// Manual-reset event the recorder waits on between samples; setting it stops the recorder.
fn stop_event() -> Result<HANDLE> {
    Ok(unsafe { CreateEventW(None, true, false, &HSTRING::from(RECORDER_STOP_EVENT))? })
}
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\spanner";

pub struct System;

impl Paths for System {
    fn data_dir() -> Result<PathBuf> {
        paths::data_dir()
    }

    fn user_dir() -> Result<PathBuf> {
        paths::user_dir()
    }
}

impl Integration for System {
    fn install() -> Result<()> {
        let _com = Com::init(COINIT_APARTMENTTHREADED);
        let exe = env::current_exe()?;
        fs::create_dir_all(Self::history_dir()?)?;
        let cfg = Self::config_file()?;
        if !cfg.exists() {
            RecorderConfig::default().save(&cfg)?;
        }
        register_task(&exe)?;
        // Fails harmlessly if an earlier install's recorder is already running.
        let _ = schtasks(&["/Run", "/TN", TASK_NAME]);
        register_uninstall(&exe)?;
        create_shortcut(&exe, &paths::shortcut()?)
    }

    fn uninstall() -> Result<Vec<String>> {
        let mut problems = Vec::new();
        let _ = Self::stop_recorder();
        let _ = schtasks(&["/End", "/TN", TASK_NAME]);
        if let Err(e) = schtasks(&["/Delete", "/TN", TASK_NAME, "/F"]) {
            problems.push(e.0);
        }
        let rc = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(UNINSTALL_KEY)) };
        if rc.0 != 0 && rc != ERROR_FILE_NOT_FOUND {
            problems.push(format!("Installed apps entry not removed (error {}).", rc.0));
        }
        for path in [paths::shortcut()?, paths::data_dir()?] {
            if let Err(e) = remove_with_retry(&path) {
                problems.push(format!("{}: {e}", path.display()));
            }
        }
        Ok(problems)
    }

    fn recorder_installed() -> bool {
        schtasks(&["/Query", "/TN", TASK_NAME]).is_ok()
    }

    fn recorder_running() -> bool {
        // The recorder holds this mutex for its lifetime (see `recorder::run`).
        match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &HSTRING::from(RECORDER_MUTEX)) } {
            Ok(h) => {
                let _ = unsafe { CloseHandle(h) };
                true
            }
            Err(_) => false,
        }
    }

    fn start_recorder() -> Result<()> {
        if Self::recorder_running() {
            return Ok(());
        }
        // Through the task, so the recorder runs elevated even when asked by the unelevated window; the
        // signed-in user may run a task registered for them.
        schtasks(&["/Run", "/TN", TASK_NAME]).map(drop)
    }

    fn stop_recorder() -> Result<()> {
        let event = stop_event()?;
        let res = unsafe { SetEvent(event) };
        let _ = unsafe { CloseHandle(event) };
        Ok(res?)
    }

    fn wait_for_stop(timeout: std::time::Duration) -> bool {
        thread_local! {
            // Opened once per recorder and reset, so a stop requested before it started doesn't apply.
            static EVENT: Option<HANDLE> = stop_event().ok().inspect(|&e| { let _ = unsafe { ResetEvent(e) }; });
        }
        match EVENT.with(|e| *e) {
            Some(e) => {
                let ms = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
                unsafe { WaitForSingleObject(e, ms) == WAIT_OBJECT_0 }
            }
            None => {
                std::thread::sleep(timeout);
                false
            }
        }
    }

    fn starts_at_sign_in() -> bool {
        // Task-level <Enabled> sits in <Settings>; it's absent when enabled. XML avoids localised list output.
        schtasks(&["/Query", "/TN", TASK_NAME, "/XML"]).is_ok_and(|xml| {
            xml.split("<Settings>").nth(1).is_none_or(|s| !s.split("</Settings>").next().unwrap_or("").contains("<Enabled>false</Enabled>"))
        })
    }

    fn set_start_at_sign_in(on: bool) -> Result<()> {
        schtasks(&["/Change", "/TN", TASK_NAME, if on { "/ENABLE" } else { "/DISABLE" }]).map(drop)
    }

    fn schedule_profile_cleanup() {
        let Ok(dir) = paths::user_dir() else { return };
        // Detached shell waits ~2 s for this process to exit and release the profile.
        let cmd = format!("ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", dir.display());
        let _ = Command::new("cmd.exe").args(["/d", "/c", &cmd]).creation_flags(CREATE_NO_WINDOW.0).spawn();
    }

    fn single_instance(name: &str) -> Option<InstanceGuard> {
        struct Mutex(HANDLE);
        impl Drop for Mutex {
            fn drop(&mut self) {
                let _ = unsafe { CloseHandle(self.0) };
            }
        }
        let handle = unsafe { CreateMutexW(None, true, &HSTRING::from(format!("Global\\{name}"))) }.ok()?;
        let guard = Mutex(handle);
        (unsafe { GetLastError() } != ERROR_ALREADY_EXISTS).then(|| InstanceGuard::new(guard))
    }

    fn lower_priority() {
        let _ = unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) };
    }

    fn is_elevated() -> bool {
        unsafe {
            let mut token = HANDLE::default();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
                return false;
            }
            let mut e = TOKEN_ELEVATION::default();
            let mut len = 0;
            let ok = GetTokenInformation(token, TokenElevation, Some((&raw mut e).cast()), size_of::<TOKEN_ELEVATION>() as u32, &mut len).is_ok();
            let _ = CloseHandle(token);
            ok && e.TokenIsElevated != 0
        }
    }

    fn run_elevated(arg: &str) -> Result<u32> {
        let exe = HSTRING::from(env::current_exe()?.as_os_str());
        let params = HSTRING::from(arg);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(exe.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        unsafe {
            ShellExecuteExW(&mut info).map_err(|e| Error(format!("Windows didn't start the elevated step: {e}")))?;
            let process = info.hProcess;
            WaitForSingleObject(process, INFINITE);
            let mut code = 0;
            let res = GetExitCodeProcess(process, &mut code);
            let _ = CloseHandle(process);
            res?;
            Ok(code)
        }
    }

    fn serve_broker(handle: impl FnMut(&str) -> String) -> Result<()> {
        super::pipe::serve(handle)
    }

    fn broker_call(msg: &str) -> Result<String> {
        super::pipe::call(msg, 5000)
    }

    fn activate_existing_window(title: &str) -> bool {
        super::window::activate(title)
    }
}

fn schtasks(args: &[&str]) -> Result<String> {
    let out = Command::new("schtasks.exe").args(args).creation_flags(CREATE_NO_WINDOW.0).output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(Error(format!("schtasks {}: {}", args[0], String::from_utf8_lossy(&out.stderr).trim())))
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// At logon, highest privileges (EC access), priority 7 (below normal), no time limit, on battery too,
/// restarted on failure.
fn task_xml(exe: &Path, user: &str) -> String {
    let exe = xml_escape(&exe.display().to_string());
    let user = xml_escape(user);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Records CPU, GPU, fan and power metrics for spanner.</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <Hidden>true</Hidden>
    <RestartOnFailure><Interval>PT1M</Interval><Count>5</Count></RestartOnFailure>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command><Arguments>--record</Arguments></Exec></Actions>
</Task>
"#
    )
}

fn current_user() -> String {
    let domain = env::var("USERDOMAIN").unwrap_or_default();
    let user = env::var("USERNAME").unwrap_or_default();
    if domain.is_empty() { user } else { format!("{domain}\\{user}") }
}

fn register_task(exe: &Path) -> Result<()> {
    // schtasks /XML requires UTF-16 with a BOM.
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(task_xml(exe, &current_user()).encode_utf16().flat_map(u16::to_le_bytes));
    let tmp = env::temp_dir().join(format!("spanner_task_{}.xml", std::process::id()));
    fs::write(&tmp, bytes)?;
    let res = schtasks(&["/Create", "/TN", TASK_NAME, "/XML", &tmp.display().to_string(), "/F"]);
    let _ = fs::remove_file(&tmp);
    res.map(drop)
}

fn reg_set(key: HKEY, name: &str, kind: windows::Win32::System::Registry::REG_VALUE_TYPE, data: &[u8]) -> Result<()> {
    win32(unsafe { RegSetValueExW(key, &HSTRING::from(name), None, kind, Some(data)) }.0, "RegSetValueExW")
}

fn reg_string(key: HKEY, name: &str, value: &str) -> Result<()> {
    let data: Vec<u8> = value.encode_utf16().chain(Some(0)).flat_map(u16::to_le_bytes).collect();
    reg_set(key, name, REG_SZ, &data)
}

fn reg_dword(key: HKEY, name: &str, value: u32) -> Result<()> {
    reg_set(key, name, REG_DWORD, &value.to_le_bytes())
}

/// Per-user entry under Settings › Apps › Installed apps; its Uninstall button runs `--uninstall`.
fn register_uninstall(exe: &Path) -> Result<()> {
    let mut key = HKEY::default();
    let rc = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(UNINSTALL_KEY),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
    };
    win32(rc.0, "RegCreateKeyExW")?;
    let exe_s = exe.display().to_string();
    let size_kb = fs::metadata(exe).map_or(0, |m| (m.len() / 1024) as u32);
    let res = (|| {
        reg_string(key, "DisplayName", "spanner")?;
        reg_string(key, "DisplayVersion", env!("CARGO_PKG_VERSION"))?;
        reg_string(key, "Publisher", "spanner")?;
        reg_string(key, "DisplayIcon", &format!("{exe_s},0"))?;
        reg_string(key, "InstallLocation", &exe.parent().map(|p| p.display().to_string()).unwrap_or_default())?;
        reg_string(key, "UninstallString", &format!("\"{exe_s}\" --uninstall"))?;
        reg_dword(key, "EstimatedSize", size_kb)?;
        reg_dword(key, "NoModify", 1)?;
        reg_dword(key, "NoRepair", 1)
    })();
    let _ = unsafe { RegCloseKey(key) };
    res
}

fn create_shortcut(exe: &Path, at: &Path) -> Result<()> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        if let Some(dir) = exe.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        }
        link.SetIconLocation(&HSTRING::from(exe.as_os_str()), 0)?;
        link.SetDescription(&HSTRING::from("Tune and monitor this laptop"))?;
        link.cast::<IPersistFile>()?.Save(&HSTRING::from(at.as_os_str()), true)?;
    }
    Ok(())
}

/// The recorder may hold its history file open for a moment after `/End`.
fn remove_with_retry(path: &Path) -> std::io::Result<()> {
    let mut last = Ok(());
    for _ in 0..10 {
        last = if path.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
        match &last {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(300)),
        }
    }
    last
}

#[cfg(test)]
mod tests {
    #[test]
    fn task_xml_escapes_and_elevates() {
        let xml = super::task_xml(std::path::Path::new(r"C:\a&b\spanner.exe"), "PC\\me");
        assert!(xml.contains(r"C:\a&amp;b\spanner.exe"));
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
    }
}
