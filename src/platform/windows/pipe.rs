//! The broker channel between the unelevated window and the elevated recorder: a local named pipe in
//! message mode, one request and one reply per connection.
//!
//! - Only the signed-in user, SYSTEM and Administrators may open it, and remote clients are rejected.
//! - The server keeps one pipe instance for its whole life (disconnect, then wait again), so the name is
//!   never free for another process to claim between clients.
//! - The client checks that the process serving the pipe is this same executable before trusting replies.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_MORE_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, FlushFileBuffers, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId, PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_MESSAGE, PIPE_WAIT, SetNamedPipeHandleState, WaitNamedPipeW,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::{HSTRING, PWSTR};

use crate::error::{Error, Result};

/// Replies (settings, live readings, hardware description) are tens of kilobytes; this bounds a bad peer.
const MAX_MESSAGE: usize = 4 * 1024 * 1024;
const BUFFER: u32 = 64 * 1024;

struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// The signed-in user's SID as a string, from this process's token (an elevated token keeps the same user).
fn user_sid() -> Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let token = Owned(token);
        let mut len = 0;
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        GetTokenInformation(token.0, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len)?;
        let user = &*(buf.as_ptr().cast::<TOKEN_USER>());
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s)?;
        let out = s.to_string().map_err(|e| Error(e.to_string()));
        let _ = LocalFree(Some(HLOCAL(s.0.cast())));
        out
    }
}

/// Per user, so two people signed in at once never share a broker.
fn pipe_name() -> Result<HSTRING> {
    Ok(HSTRING::from(format!(r"\\.\pipe\spanner-broker-{}", user_sid()?)))
}

fn read_message(h: HANDLE) -> Result<String> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; BUFFER as usize];
    loop {
        let mut n = 0;
        let res = unsafe { ReadFile(h, Some(&mut buf), Some(&mut n), None) };
        out.extend_from_slice(&buf[..n as usize]);
        if out.len() > MAX_MESSAGE {
            return Err(Error::msg("broker message too large"));
        }
        match res {
            Ok(()) => break,
            Err(e) if e.code() == ERROR_MORE_DATA.to_hresult() => continue,
            Err(e) => return Err(e.into()),
        }
    }
    String::from_utf8(out).map_err(|_| Error::msg("broker message isn't UTF-8"))
}

fn write_message(h: HANDLE, msg: &str) -> Result<()> {
    if msg.len() > MAX_MESSAGE {
        return Err(Error::msg("broker message too large"));
    }
    let mut n = 0;
    unsafe { WriteFile(h, Some(msg.as_bytes()), Some(&mut n), None)? };
    Ok(())
}

/// Serves requests until the process ends: one message in, `handle`'s reply out, per connection.
pub fn serve(handle: impl FnMut(&str) -> String) -> Result<()> {
    serve_on(&pipe_name()?, handle)
}

fn serve_on(name: &HSTRING, mut handle: impl FnMut(&str) -> String) -> Result<()> {
    // Full access for SYSTEM, Administrators and this user only; P blocks inherited entries.
    let sddl = HSTRING::from(format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{})", user_sid()?));
    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(&sddl, SDDL_REVISION_1, &mut sd, None)? };
    let attrs = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: false.into() };
    let pipe = unsafe {
        CreateNamedPipeW(
            name,
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            BUFFER,
            BUFFER,
            0,
            Some(&attrs),
        )
    };
    let _ = unsafe { LocalFree(Some(HLOCAL(sd.0))) };
    if pipe.is_invalid() {
        return Err(Error(format!("couldn't create the broker pipe: {}", windows::core::Error::from_thread())));
    }
    let pipe = Owned(pipe);
    loop {
        let connected = match unsafe { ConnectNamedPipe(pipe.0, None) } {
            Ok(()) => true,
            Err(e) => e.code() == ERROR_PIPE_CONNECTED.to_hresult(),
        };
        if connected && let Ok(req) = read_message(pipe.0) {
            let reply = handle(&req);
            if write_message(pipe.0, &reply).is_ok() {
                let _ = unsafe { FlushFileBuffers(pipe.0) };
            }
        }
        let _ = unsafe { DisconnectNamedPipe(pipe.0) };
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase();
    norm(a) == norm(b)
}

fn process_image(pid: u32) -> Result<PathBuf> {
    unsafe {
        let p = Owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?);
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(p.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)?;
        Ok(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// Sends one request and returns the reply. Fails fast when no broker is listening.
pub fn call(msg: &str, wait_ms: u32) -> Result<String> {
    call_on(&pipe_name()?, msg, wait_ms)
}

fn call_on(name: &HSTRING, msg: &str, wait_ms: u32) -> Result<String> {
    let h = loop {
        match unsafe { CreateFileW(name, (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_NONE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) } {
            Ok(h) => break Owned(h),
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() => {
                // The broker answers one request at a time; wait for it to be free.
                unsafe { WaitNamedPipeW(name, wait_ms).ok()? };
            }
            Err(e) => return Err(Error(format!("the background service isn't reachable ({e})"))),
        }
    };
    let mode = PIPE_READMODE_MESSAGE;
    unsafe { SetNamedPipeHandleState(h.0, Some(&mode), None, None)? };
    let mut pid = 0;
    unsafe { GetNamedPipeServerProcessId(h.0, &mut pid)? };
    let me = std::env::current_exe()?;
    if !same_file(&process_image(pid)?, &me) {
        return Err(Error::msg("the background service pipe is held by another program"));
    }
    write_message(h.0, msg)?;
    read_message(h.0)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use windows::core::HSTRING;

    #[test]
    fn round_trips_messages_over_one_server_instance() {
        let name = HSTRING::from(format!(r"\\.\pipe\spanner-test-{}", std::process::id()));
        let server = name.clone();
        std::thread::spawn(move || super::serve_on(&server, |m| format!("echo:{}", m.len())));
        let mut reply = Err(crate::error::Error::msg("not started"));
        for _ in 0..100 {
            reply = super::call_on(&name, "hello", 1000);
            if reply.is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(reply.unwrap(), "echo:5");
        // Larger than one read buffer, and a second client on the same server instance.
        assert_eq!(super::call_on(&name, &"x".repeat(200_000), 1000).unwrap(), "echo:200000");
        assert_eq!(super::call_on(&name, "again", 1000).unwrap(), "echo:5");
    }

    #[test]
    fn has_a_user_sid() {
        assert!(super::user_sid().unwrap().starts_with("S-1-"));
    }
}
