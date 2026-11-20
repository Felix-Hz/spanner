//! EC transport over MSI's ACPI WMI provider (`root\WMI:MSI_ACPI`), the path MSI Center uses.
//!
//! Wire format, `Package_32` (32 bytes) in and out:
//! - `Get_Data([addr])`        -> `[status, value, ..]`
//! - `Set_Data([addr, value])` -> `[status, ..]`
//! - `Get_EC()`                -> `[status, flags, version[12], date[8], time[8]]`
//!
//! `status == 1` is success. `Get_Data` semantics were verified on this machine (0xA0.. reads back the
//! firmware string). `Set_Data` is inferred from it, so `write` reads back to confirm.

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoSetProxyBlanket, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Wmi::{
    IEnumWbemClassObject, IWbemClassObject, IWbemLocator, IWbemServices, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
    WBEM_FLAG_RETURN_WBEM_COMPLETE, WBEM_INFINITE, WbemLocator,
};
use windows::core::{BSTR, HSTRING, Interface, PCWSTR, w};

use super::com::Var;
use crate::error::{Error, Result};
use crate::platform::EcTransport;

const RPC_C_AUTHN_WINNT: u32 = 10;
const RPC_C_AUTHZ_NONE: u32 = 0;

struct Method {
    name: BSTR,
    input: Option<IWbemClassObject>,
}

pub struct WmiEc {
    svc: IWbemServices,
    instance: BSTR,
    package: IWbemClassObject,
    get_data: Method,
    set_data: Method,
    firmware: String,
}

impl WmiEc {
    /// COM must be initialised on this thread; the value is bound to it.
    pub fn open() -> Result<Self> {
        unsafe {
            let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
            let svc = locator.ConnectServer(&BSTR::from("ROOT\\WMI"), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None)?;
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

            let class = get_object(&svc, "MSI_ACPI")?;
            let method = |name: &str| -> Result<Method> {
                let (mut input, mut output) = (None, None);
                class.GetMethod(&HSTRING::from(name), 0, &mut input, &mut output)?;
                Ok(Method { name: BSTR::from(name), input })
            };
            let mut ec = Self {
                instance: first_instance_path(&svc, "MSI_ACPI")?,
                package: get_object(&svc, "Package_32")?,
                get_data: method("Get_Data")?,
                set_data: method("Set_Data")?,
                svc,
                firmware: String::new(),
            };
            let info = ec.call(&method("Get_EC")?, None)?;
            ec.firmware = info[2..14].iter().take_while(|c| c.is_ascii_graphic()).map(|&c| c as char).collect();
            Ok(ec)
        }
    }

    fn call(&self, m: &Method, payload: Option<[u8; 32]>) -> Result<[u8; 32]> {
        unsafe {
            let input = match (&m.input, payload) {
                (Some(sig), Some(bytes)) => {
                    let args = sig.SpawnInstance(0)?;
                    let pkg = self.package.SpawnInstance(0)?;
                    pkg.Put(w!("Bytes"), 0, &Var::from_bytes(&bytes)?.0, 0)?;
                    args.Put(w!("Data"), 0, &Var::from_unknown(pkg.cast()?).0, 0)?;
                    Some(args)
                }
                (None, None) => None,
                _ => return Err(Error(format!("{}: argument mismatch", m.name))),
            };
            let mut out = None;
            self.svc.ExecMethod(&self.instance, &m.name, WBEM_FLAG_RETURN_WBEM_COMPLETE, None, input.as_ref(), Some(&mut out), None)?;
            let out = out.ok_or_else(|| Error(format!("{}: no output", m.name)))?;
            let mut data = Var::empty();
            out.Get(w!("Data"), 0, &mut data.0, None, None)?;
            let pkg: IWbemClassObject = data.to_interface()?;
            let mut bytes = Var::empty();
            pkg.Get(w!("Bytes"), 0, &mut bytes.0, None, None)?;
            let bytes = bytes.to_bytes()?;
            let mut res = [0u8; 32];
            let n = bytes.len().min(32);
            res[..n].copy_from_slice(&bytes[..n]);
            Ok(res)
        }
    }
}

impl EcTransport for WmiEc {
    fn firmware(&self) -> &str {
        &self.firmware
    }

    fn read(&self, address: u8) -> Result<u8> {
        let mut p = [0u8; 32];
        p[0] = address;
        match self.call(&self.get_data, Some(p))? {
            [1, value, ..] => Ok(value),
            [status, ..] => Err(Error(format!("EC read {address:#04x}: status {status}"))),
        }
    }

    fn write(&self, address: u8, value: u8) -> Result<()> {
        let mut p = [0u8; 32];
        p[0] = address;
        p[1] = value;
        let status = self.call(&self.set_data, Some(p))?[0];
        if status != 1 {
            return Err(Error(format!("EC write {address:#04x}: status {status}")));
        }
        match self.read(address)? {
            v if v == value => Ok(()),
            v => Err(Error(format!("EC write {address:#04x} not applied: wrote {value:#04x}, read {v:#04x}"))),
        }
    }
}

unsafe fn get_object(svc: &IWbemServices, path: &str) -> Result<IWbemClassObject> {
    let mut obj = None;
    unsafe { svc.GetObject(&BSTR::from(path), WBEM_FLAG_RETURN_WBEM_COMPLETE, None, Some(&mut obj), None)? };
    obj.ok_or_else(|| Error(format!("WMI object {path} not found")))
}

unsafe fn first_instance_path(svc: &IWbemServices, class: &str) -> Result<BSTR> {
    unsafe {
        let it: IEnumWbemClassObject = svc.ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from(format!("SELECT __RELPATH FROM {class}")),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        )?;
        let mut row = [None];
        let mut n = 0;
        it.Next(WBEM_INFINITE, &mut row, &mut n).ok()?;
        let obj = row[0].take().filter(|_| n == 1).ok_or_else(|| Error(format!("no {class} instance (MSI ACPI driver missing?)")))?;
        let mut v = Var::empty();
        obj.Get(w!("__RELPATH"), 0, &mut v.0, None, None)?;
        v.to_string_opt().map(BSTR::from).ok_or_else(|| Error(format!("{class}: no __RELPATH")))
    }
}
