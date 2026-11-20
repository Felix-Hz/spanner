//! COM lifetime and owned VARIANTs.

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{COINIT, CoInitializeEx, CoUninitialize, SAFEARRAY};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayCreateVector, SafeArrayGetLBound, SafeArrayGetUBound, SafeArrayUnaccessData,
};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_ARRAY, VT_BSTR, VT_UI1, VT_UNKNOWN, VariantClear};
use windows::core::{IUnknown, Interface};

use crate::error::{Error, Result};

const VT_UI1_ARRAY: VARENUM = VARENUM(VT_ARRAY.0 | VT_UI1.0);

/// Per-thread COM initialisation. If the thread already uses another apartment model, that one is kept
/// and nothing is uninitialised on drop.
pub struct Com {
    owned: bool,
}

impl Com {
    pub fn init(model: COINIT) -> Self {
        let hr = unsafe { CoInitializeEx(None, model) };
        Self { owned: hr.is_ok() && hr != RPC_E_CHANGED_MODE }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.owned {
            unsafe { CoUninitialize() };
        }
    }
}

/// Owned VARIANT; `VariantClear` on drop releases interfaces and arrays.
pub struct Var(pub VARIANT);

impl Var {
    pub fn empty() -> Self {
        Self(VARIANT::default())
    }

    fn vt(&self) -> VARENUM {
        unsafe { self.0.Anonymous.Anonymous.vt }
    }

    pub fn from_unknown(u: IUnknown) -> Self {
        let mut v = VARIANT::default();
        unsafe {
            let inner = &mut *v.Anonymous.Anonymous;
            inner.vt = VT_UNKNOWN;
            inner.Anonymous.punkVal = ManuallyDrop::new(Some(u));
        }
        Self(v)
    }

    /// WMI `uint8[]`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        unsafe {
            let psa = SafeArrayCreateVector(VT_UI1, 0, bytes.len() as u32);
            if psa.is_null() {
                return Err(Error::msg("SafeArrayCreateVector returned null"));
            }
            let mut data = std::ptr::null_mut();
            SafeArrayAccessData(psa, &mut data)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), data.cast::<u8>(), bytes.len());
            SafeArrayUnaccessData(psa)?;
            let mut v = VARIANT::default();
            let inner = &mut *v.Anonymous.Anonymous;
            inner.vt = VT_UI1_ARRAY;
            inner.Anonymous.parray = psa;
            Ok(Self(v))
        }
    }

    pub fn to_interface<T: Interface>(&self) -> Result<T> {
        if self.vt() != VT_UNKNOWN {
            return Err(Error(format!("expected an object, got VARIANT type {}", self.vt().0)));
        }
        let unk: &Option<IUnknown> = unsafe { &self.0.Anonymous.Anonymous.Anonymous.punkVal };
        Ok(unk.as_ref().ok_or_else(|| Error::msg("null object"))?.cast()?)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        if self.vt() != VT_UI1_ARRAY {
            return Err(Error(format!("expected uint8[], got VARIANT type {}", self.vt().0)));
        }
        unsafe {
            let psa: *mut SAFEARRAY = self.0.Anonymous.Anonymous.Anonymous.parray;
            let len = (SafeArrayGetUBound(psa, 1)? - SafeArrayGetLBound(psa, 1)? + 1).max(0) as usize;
            let mut data = std::ptr::null_mut();
            SafeArrayAccessData(psa, &mut data)?;
            let out = std::slice::from_raw_parts(data.cast::<u8>(), len).to_vec();
            SafeArrayUnaccessData(psa)?;
            Ok(out)
        }
    }

    pub fn to_string_opt(&self) -> Option<String> {
        (self.vt() == VT_BSTR).then(|| unsafe { self.0.Anonymous.Anonymous.Anonymous.bstrVal.to_string() })
    }
}

impl Drop for Var {
    fn drop(&mut self) {
        let _ = unsafe { VariantClear(&mut self.0) };
    }
}
