use std::fmt;

/// Crate-wide error: a user-facing message.
#[derive(Debug)]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn msg(text: impl Into<String>) -> Self {
        Self(text.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self(format!("I/O: {e}"))
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self(format!("JSON: {e}"))
    }
}

#[cfg(windows)]
impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        Self(format!("Windows: {} ({:#010x})", e.message(), e.code().0))
    }
}

/// Win32 status code to `Result`.
#[cfg(windows)]
pub fn win32(code: u32, call: &str) -> Result<()> {
    if code == 0 { Ok(()) } else { Err(Error(format!("{call} failed: Win32 error {code}"))) }
}
