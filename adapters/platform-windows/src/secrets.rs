//! Secrets on Windows, never in ordinary configuration: the same calls as
//! the Linux adapter's `secrets` (the Secret Service there), kept in the
//! Windows Credential Manager (`os::credentials`).
//!
//! A secret `name` is the generic credential `Narrivy/<name>`. Values are
//! never logged, and errors carry no content.

#![forbid(unsafe_code)]

/// Why a secret could not be read or kept. Content-free. The variants are
/// the Linux adapter's; Windows has no locked keyring, so `Locked` is never
/// returned here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretError {
    /// No credential store on this system or in this logon session.
    Unavailable,
    /// The store is locked.
    Locked,
    /// The store refused the request, or the value is not acceptable.
    Failed,
}

/// The largest value kept: the Credential Manager's limit for a generic
/// credential (`CRED_MAX_CREDENTIAL_BLOB_SIZE`).
pub const MAX_VALUE_BYTES: usize = 2560;
const TARGET_PREFIX: &str = "Narrivy/";

/// The credential name for the secret `name`: `Narrivy/<name>`, for a
/// short plain name only.
fn target(name: &str) -> Result<String, SecretError> {
    let plain = !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if plain {
        Ok(format!("{TARGET_PREFIX}{name}"))
    } else {
        Err(SecretError::Failed)
    }
}

/// The secret `name`, if the Credential Manager has it.
///
/// # Errors
/// [`SecretError`] when the name is not plain or the store fails.
pub fn read(name: &str) -> Result<Option<String>, SecretError> {
    let target = target(name)?;
    #[cfg(windows)]
    return crate::os::credentials::read(&target);
    #[cfg(not(windows))]
    {
        let _ = target;
        Err(SecretError::Unavailable)
    }
}

/// Keep `value` as the secret `name` (replacing it); `label` is shown
/// beside it in Credential Manager.
///
/// # Errors
/// [`SecretError`] when the name is not plain, the value is empty or too
/// large, or the store fails.
pub fn store(name: &str, label: &str, value: &str) -> Result<(), SecretError> {
    let target = target(name)?;
    if value.is_empty() || value.len() > MAX_VALUE_BYTES {
        return Err(SecretError::Failed);
    }
    #[cfg(windows)]
    return crate::os::credentials::store(&target, label, value);
    #[cfg(not(windows))]
    {
        let _ = (target, label);
        Err(SecretError::Unavailable)
    }
}

/// Remove the secret `name`; one that is not there is not an error.
///
/// # Errors
/// [`SecretError`] when the name is not plain or the store fails.
pub fn remove(name: &str) -> Result<(), SecretError> {
    let target = target(name)?;
    #[cfg(windows)]
    return crate::os::credentials::remove(&target);
    #[cfg(not(windows))]
    {
        let _ = target;
        Err(SecretError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_names_are_short_and_plain() {
        assert_eq!(
            target("brave-search-key").as_deref(),
            Ok("Narrivy/brave-search-key")
        );
        for bad in ["", "a/b", "a b", "..\\x", "é", &"a".repeat(129)] {
            assert_eq!(target(bad), Err(SecretError::Failed), "{bad:?}");
        }
    }

    #[test]
    fn empty_or_oversized_values_are_refused() {
        assert_eq!(store("x", "x", ""), Err(SecretError::Failed));
        assert_eq!(
            store("x", "x", &"a".repeat(MAX_VALUE_BYTES + 1)),
            Err(SecretError::Failed)
        );
    }

    /// The real Credential Manager: write, read back, replace, remove.
    /// Needs a desktop logon (a key-authenticated SSH session has no
    /// credential store): `cargo test -p narrivy-platform-windows secrets
    /// -- --ignored` from the signed-in user's session.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs a desktop logon session with a credential store"]
    fn a_secret_round_trips_through_the_credential_manager() {
        let name = format!("test-{}", std::process::id());
        assert_eq!(read(&name), Ok(None));
        assert_eq!(store(&name, "Narrivy test", "first"), Ok(()));
        assert_eq!(read(&name), Ok(Some("first".to_owned())));
        assert_eq!(store(&name, "Narrivy test", "second"), Ok(()));
        assert_eq!(read(&name), Ok(Some("second".to_owned())));
        assert_eq!(remove(&name), Ok(()));
        assert_eq!(read(&name), Ok(None));
        assert_eq!(remove(&name), Ok(()));
    }
}
