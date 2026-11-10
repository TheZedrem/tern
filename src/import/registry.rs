//! PuTTY's and WinSCP's sessions read straight from the Windows registry
//! (spec 0015, OQ-029): the subkeys of one key under `HKEY_CURRENT_USER`,
//! each asked by name for the values [`Session::wanted`] lists. Any other
//! value, `Password` among them, is never queried.

use super::MAX_BYTES;
use super::sessions::Session;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey,
    RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};

/// An open registry key, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RegOpenKeyExW.
        unsafe { RegCloseKey(self.0) };
    }
}

/// `text` as a NUL-terminated UTF-16 string.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// The sessions under `HKEY_CURRENT_USER\<under>`, in the registry's
/// (alphabetical) order. No such key is no sessions; a key that can't be
/// read, or over [`MAX_BYTES`] of values, is an error.
pub fn sessions(under: &str) -> Result<Vec<Session>, String> {
    let fail = |code: u32| format!("HKEY_CURRENT_USER\\{under}: Windows error {code}");
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: the path is NUL-terminated; `key` is written on success only.
    let code = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(under).as_ptr(),
            0,
            KEY_READ,
            &mut key,
        )
    };
    match code {
        ERROR_SUCCESS => {}
        ERROR_FILE_NOT_FOUND => return Ok(Vec::new()),
        code => return Err(fail(code)),
    }
    let key = Key(key);
    let mut sessions = Vec::new();
    let mut total = 0;
    for index in 0.. {
        // Key names are at most 255 characters.
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        // SAFETY: `name` holds `len` characters; the optional out-parameters are null.
        let code = unsafe {
            RegEnumKeyExW(
                key.0,
                index,
                name.as_mut_ptr(),
                &mut len,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        match code {
            ERROR_SUCCESS => {}
            windows_sys::Win32::Foundation::ERROR_NO_MORE_ITEMS => break,
            code => return Err(fail(code)),
        }
        let name = String::from_utf16_lossy(&name[..len as usize]);
        let mut session = Session::new(&name);
        let subkey = wide(&name);
        for &value in Session::wanted() {
            if let Some(text) = read_value(&key, &subkey, value).map_err(fail)? {
                total += text.len() as u64;
                session.set(value, text);
            }
        }
        if total > MAX_BYTES {
            return Err(format!("HKEY_CURRENT_USER\\{under} holds over 10 MB"));
        }
        sessions.push(session);
    }
    Ok(sessions)
}

/// The string or DWORD `value` of `subkey`, as text; `None` if it isn't set
/// (or is of another type).
fn read_value(key: &Key, subkey: &[u16], value: &str) -> Result<Option<String>, u32> {
    let value = wide(value);
    let mut buffer = vec![0u16; 256];
    loop {
        let mut kind = 0;
        let mut bytes = (buffer.len() * 2) as u32;
        // SAFETY: both names are NUL-terminated; `buffer` holds `bytes` bytes.
        let code = unsafe {
            RegGetValueW(
                key.0,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ | RRF_RT_REG_DWORD,
                &mut kind,
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        match code {
            ERROR_SUCCESS if kind == REG_DWORD => {
                let low = u32::from(buffer[0]);
                let high = u32::from(buffer[1]);
                return Ok(Some((low | high << 16).to_string()));
            }
            ERROR_SUCCESS => {
                let chars = &buffer[..bytes as usize / 2];
                let text = String::from_utf16_lossy(chars);
                return Ok(Some(text.trim_end_matches('\0').to_owned()));
            }
            ERROR_MORE_DATA => buffer.resize((bytes as usize).div_ceil(2) + 1, 0),
            // Not set, or a type the parsers don't read.
            ERROR_FILE_NOT_FOUND | windows_sys::Win32::Foundation::ERROR_UNSUPPORTED_TYPE => {
                return Ok(None);
            }
            code => return Err(code),
        }
    }
}
