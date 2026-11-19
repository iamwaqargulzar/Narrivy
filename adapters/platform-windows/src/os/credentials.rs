//! Secrets in the Windows Credential Manager: generic credentials of the
//! signed-in user, kept on this computer only (`CRED_PERSIST_LOCAL_MACHINE`:
//! not copied to other computers with a roaming profile). Windows protects
//! them with the user's logon secret; only processes running as that user
//! can read them. The rules (names, sizes) are in [`crate::secrets`].

use windows::Win32::Foundation::{ERROR_NO_SUCH_LOGON_SESSION, ERROR_NOT_FOUND};
use windows::Win32::Security::Credentials::{
    CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
    CredReadW, CredWriteW,
};
use windows::core::{PCWSTR, PWSTR};

use crate::secrets::SecretError;

/// Wide, NUL-terminated copy of `text`.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn not_found(error: &windows::core::Error) -> bool {
    error.code() == ERROR_NOT_FOUND.to_hresult()
}

/// A logon without a credential store (a key-authenticated SSH session, a
/// service without a loaded profile) is `Unavailable`; anything else
/// `Failed`.
fn failure(error: &windows::core::Error) -> SecretError {
    if error.code() == ERROR_NO_SUCH_LOGON_SESSION.to_hresult() {
        SecretError::Unavailable
    } else {
        SecretError::Failed
    }
}

/// The credential `target`'s value, `None` when there is none.
pub fn read(target: &str) -> Result<Option<String>, SecretError> {
    let target = wide(target);
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `target` is NUL-terminated and outlives the call; on success
    // Windows sets `credential` to a block it allocated, freed below.
    let read = unsafe {
        CredReadW(
            PCWSTR(target.as_ptr()),
            CRED_TYPE_GENERIC,
            None,
            &raw mut credential,
        )
    };
    match read {
        Ok(()) => {}
        Err(error) if not_found(&error) => return Ok(None),
        Err(error) => return Err(failure(&error)),
    }
    if credential.is_null() {
        return Err(SecretError::Failed);
    }
    // SAFETY: `credential` points at the block `CredReadW` returned; its
    // blob is `CredentialBlobSize` bytes (or null when empty) and stays
    // valid until `CredFree`, which runs once, after the bytes are copied.
    let bytes = unsafe {
        let block = &*credential;
        let bytes = if block.CredentialBlob.is_null() || block.CredentialBlobSize == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(block.CredentialBlob, block.CredentialBlobSize as usize)
                .to_vec()
        };
        CredFree(credential.cast());
        bytes
    };
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| SecretError::Failed)
}

/// Keep `value` as the credential `target`, replacing any earlier one.
/// `comment` is what Credential Manager shows beside it.
pub fn store(target: &str, comment: &str, value: &str) -> Result<(), SecretError> {
    let mut target = wide(target);
    let mut comment = wide(comment);
    let mut blob = value.as_bytes().to_vec();
    let size = u32::try_from(blob.len()).map_err(|_| SecretError::Failed)?;
    let credential = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_mut_ptr()),
        Comment: PWSTR(comment.as_mut_ptr()),
        CredentialBlobSize: size,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..CREDENTIALW::default()
    };
    // SAFETY: every pointer in `credential` points into a buffer above that
    // outlives the call (names NUL-terminated, the blob `size` bytes);
    // Windows copies them and keeps no reference.
    let written = unsafe { CredWriteW(&raw const credential, 0) };
    // The value's bytes are not left in this process's freed memory.
    blob.fill(0);
    written.map_err(|error| failure(&error))
}

/// Remove the credential `target`; none there is not an error.
pub fn remove(target: &str) -> Result<(), SecretError> {
    let target = wide(target);
    // SAFETY: `target` is NUL-terminated and outlives the call.
    match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
        Ok(()) => Ok(()),
        Err(error) if not_found(&error) => Ok(()),
        Err(error) => Err(failure(&error)),
    }
}
