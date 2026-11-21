//! Read-only WMI queries (`ROOT\CIMV2`, `ROOT\WMI`) for hardware information and battery state.

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoSetProxyBlanket, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Wmi::{IWbemLocator, IWbemServices, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE, WbemLocator};
use windows::core::{BSTR, HSTRING, PCWSTR};

use super::com::Var;
use crate::error::Result;

const RPC_C_AUTHN_WINNT: u32 = 10;
const RPC_C_AUTHZ_NONE: u32 = 0;
/// Upper bound on rows read from one query; hardware classes return a handful.
const MAX_ROWS: usize = 64;

pub struct Wmi {
    svc: IWbemServices,
}

impl Wmi {
    /// COM must be initialised on this thread; the value is bound to it.
    pub fn connect(namespace: &str) -> Result<Self> {
        unsafe {
            let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
            let svc = locator.ConnectServer(&BSTR::from(namespace), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None)?;
            CoSetProxyBlanket(
                &svc,
                RPC_C_AUTHN_WINNT,
                RPC_C_AUTHZ_NONE,
                PCWSTR::null(),
                RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
            )?;
            Ok(Self { svc })
        }
    }

    /// `SELECT fields FROM class [WHERE ...]`, each row as the named properties in order. A property that
    /// is missing or null comes back as an empty VARIANT, so callers read it as `None`.
    pub fn rows(&self, class: &str, fields: &[&str], filter: Option<&str>) -> Result<Vec<Vec<Var>>> {
        let wql = match filter {
            Some(f) => format!("SELECT {} FROM {class} WHERE {f}", fields.join(", ")),
            None => format!("SELECT {} FROM {class}", fields.join(", ")),
        };
        let mut out = Vec::new();
        unsafe {
            let it = self.svc.ExecQuery(&BSTR::from("WQL"), &BSTR::from(wql), WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, None)?;
            while out.len() < MAX_ROWS {
                let mut row = [None];
                let mut n = 0;
                if it.Next(WBEM_INFINITE, &mut row, &mut n).is_err() || n == 0 {
                    break;
                }
                let Some(obj) = row[0].take() else { break };
                let values = fields
                    .iter()
                    .map(|f| {
                        let mut v = Var::empty();
                        let _ = obj.Get(&HSTRING::from(*f), 0, &mut v.0, None, None);
                        v
                    })
                    .collect();
                out.push(values);
            }
        }
        Ok(out)
    }
}
