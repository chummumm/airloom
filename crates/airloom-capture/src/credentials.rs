//! Per-user Windows Credential Manager storage.
//!
//! This module never decides to remember a credential. Call `save` only after
//! explicit user consent. Callers own (and should zeroize) returned plaintext
//! bytes. There is no filesystem or non-Windows plaintext fallback.

use crate::{CaptureError, Result};

/// Saves opaque authentication bytes for the current Windows user.
pub fn save(service: &str, key: &str, secret: &[u8]) -> Result<()> {
    #[cfg(windows)]
    {
        implementation::save(service, key, secret)
    }
    #[cfg(not(windows))]
    {
        let _ = (service, key, secret);
        Err(CaptureError::UnsupportedPlatform)
    }
}

/// Returns `None` when no credential has been saved under this key.
pub fn load(service: &str, key: &str) -> Result<Option<Vec<u8>>> {
    #[cfg(windows)]
    {
        implementation::load(service, key)
    }
    #[cfg(not(windows))]
    {
        let _ = (service, key);
        Err(CaptureError::UnsupportedPlatform)
    }
}

/// Removes a saved credential. A missing entry is already successfully deleted.
pub fn delete(service: &str, key: &str) -> Result<()> {
    #[cfg(windows)]
    {
        implementation::delete(service, key)
    }
    #[cfg(not(windows))]
    {
        let _ = (service, key);
        Err(CaptureError::UnsupportedPlatform)
    }
}

#[cfg(any(windows, test))]
fn target_name(service: &str, key: &str) -> Result<Vec<u16>> {
    if service.is_empty()
        || key.is_empty()
        || service.chars().chain(key.chars()).any(char::is_control)
    {
        return Err(CaptureError::InvalidConfig(
            "credential service/key is empty or contains control characters",
        ));
    }
    // The service length prevents distinct (service, key) pairs containing
    // separators from aliasing to the same credential.
    let name = format!("Airloom/{}:{service}/{key}", service.encode_utf16().count());
    let mut wide: Vec<u16> = name.encode_utf16().collect();
    if wide.len() > 32_767 {
        return Err(CaptureError::InvalidConfig("credential target is too long"));
    }
    wide.push(0);
    Ok(wide)
}

#[cfg(windows)]
mod implementation {
    use super::*;
    use std::{ptr, slice};
    use windows::core::{HRESULT, PCWSTR, PWSTR};
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_MAX_CREDENTIAL_BLOB_SIZE,
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };
    use zeroize::Zeroize;

    fn failure(operation: &'static str, error: windows::core::Error) -> CaptureError {
        CaptureError::Windows {
            operation,
            code: error.code().0,
        }
    }

    pub(super) fn save(service: &str, key: &str, secret: &[u8]) -> Result<()> {
        let mut target = target_name(service, key)?;
        if secret.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
            return Err(CaptureError::CredentialTooLarge);
        }
        let mut username: Vec<u16> = "Airloom\0".encode_utf16().collect();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            CredentialBlobSize: secret.len() as u32,
            CredentialBlob: secret.as_ptr().cast_mut(),
            // This persists for the current user on this machine; it does not
            // grant access to other users and does not roam to another machine.
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(username.as_mut_ptr()),
            ..Default::default()
        };
        unsafe { CredWriteW(&credential, 0) }.map_err(|e| failure("CredWriteW", e))
    }

    struct CredentialBuffer(*mut CREDENTIALW);

    impl Drop for CredentialBuffer {
        fn drop(&mut self) {
            if self.0.is_null() {
                return;
            }
            unsafe {
                let credential = &mut *self.0;
                if !credential.CredentialBlob.is_null() {
                    slice::from_raw_parts_mut(
                        credential.CredentialBlob,
                        credential.CredentialBlobSize as usize,
                    )
                    .zeroize();
                }
                CredFree(self.0.cast());
            }
        }
    }

    pub(super) fn load(service: &str, key: &str) -> Result<Option<Vec<u8>>> {
        let target = target_name(service, key)?;
        let mut pointer = ptr::null_mut();
        match unsafe {
            CredReadW(
                PCWSTR(target.as_ptr()),
                CRED_TYPE_GENERIC,
                None,
                &mut pointer,
            )
        } {
            Ok(()) => {}
            Err(e) if e.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => return Ok(None),
            Err(e) => return Err(failure("CredReadW", e)),
        }
        let buffer = CredentialBuffer(pointer);
        if buffer.0.is_null() {
            return Err(CaptureError::Conversion(
                "Credential Manager returned an empty allocation".into(),
            ));
        }
        let credential = unsafe { &*buffer.0 };
        let length = credential.CredentialBlobSize as usize;
        if length == 0 {
            return Ok(Some(Vec::new()));
        }
        if credential.CredentialBlob.is_null() || length > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
            return Err(CaptureError::Conversion(
                "Credential Manager returned an invalid blob".into(),
            ));
        }
        Ok(Some(
            unsafe { slice::from_raw_parts(credential.CredentialBlob, length) }.to_vec(),
        ))
    }

    pub(super) fn delete(service: &str, key: &str) -> Result<()> {
        let target = target_name(service, key)?;
        match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(e) if e.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => Ok(()),
            Err(e) => Err(failure("CredDeleteW", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_names_do_not_alias_and_reject_embedded_nul() {
        assert_ne!(
            target_name("one/two", "three").unwrap(),
            target_name("one", "two/three").unwrap()
        );
        assert!(target_name("AirPlay", "bad\0key").is_err());
        assert!(target_name("", "key").is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_platform_has_no_plaintext_fallback() {
        assert!(matches!(
            save("test", "key", b"private"),
            Err(CaptureError::UnsupportedPlatform)
        ));
        assert!(matches!(
            load("test", "key"),
            Err(CaptureError::UnsupportedPlatform)
        ));
        assert!(matches!(
            delete("test", "key"),
            Err(CaptureError::UnsupportedPlatform)
        ));
    }
}
