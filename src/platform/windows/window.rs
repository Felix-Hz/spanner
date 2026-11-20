//! Finding and focusing this program's own window (single-window behaviour).

use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, TRUE};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
};
use windows::core::{BOOL, PWSTR};

struct Search {
    title: Vec<u16>,
    exe: PathBuf,
    found: Option<HWND>,
}

fn image_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = unsafe { &mut *(lparam.0 as *mut Search) };
    let mut text = [0u16; 128];
    let n = unsafe { GetWindowTextW(hwnd, &mut text) } as usize;
    if text[..n] == search.title[..] {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        // Match on the executable too, so an Explorer window named "spanner" is never picked.
        if image_path(pid).is_some_and(|p| p == search.exe) {
            search.found = Some(hwnd);
            return BOOL(0);
        }
    }
    TRUE
}

pub fn activate(title: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let mut search = Search { title: title.encode_utf16().collect(), exe, found: None };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&raw mut search as isize));
        let Some(hwnd) = search.found else { return false };
        if !IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
    true
}
