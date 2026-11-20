use std::path::PathBuf;

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_LocalAppData, FOLDERID_ProgramData, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::core::GUID;

use crate::error::{Error, Result};

pub const APP_DIR: &str = "spanner";

fn known(id: &GUID) -> Result<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None)?;
        let s = p.to_string();
        CoTaskMemFree(Some(p.0 as *const _));
        Ok(PathBuf::from(s.map_err(|e| Error(e.to_string()))?))
    }
}

/// `%ProgramData%\spanner`
pub fn data_dir() -> Result<PathBuf> {
    Ok(known(&FOLDERID_ProgramData)?.join(APP_DIR))
}

/// `%LOCALAPPDATA%\spanner`
pub fn user_dir() -> Result<PathBuf> {
    Ok(known(&FOLDERID_LocalAppData)?.join(APP_DIR))
}

pub fn shortcut() -> Result<PathBuf> {
    Ok(known(&FOLDERID_Desktop)?.join("spanner.lnk"))
}
