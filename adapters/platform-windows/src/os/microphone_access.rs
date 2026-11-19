//! Whether Windows' privacy settings let desktop programs use the
//! microphone. When they don't, capture still opens and only delivers
//! silence, so the microphone check reads these switches to say why.

use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::core::{HSTRING, w};

const CONSENT: &str =
    r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

/// True when a privacy switch blocks this program from the microphone:
/// Settings → Privacy & security → Microphone, its "Microphone access" (for
/// this computer or this user) or "Let desktop apps access your microphone".
/// A setting that can't be read counts as allowed.
#[must_use]
pub fn microphone_blocked() -> bool {
    let non_packaged = format!(r"{CONSENT}\NonPackaged");
    [
        (HKEY_LOCAL_MACHINE, CONSENT),
        (HKEY_CURRENT_USER, CONSENT),
        (HKEY_CURRENT_USER, non_packaged.as_str()),
    ]
    .into_iter()
    .any(|(root, key)| consent(root, key).is_some_and(|value| value.eq_ignore_ascii_case("Deny")))
}

/// The `Value` string ("Allow" or "Deny") under `key`, if there is one.
fn consent(root: HKEY, key: &str) -> Option<String> {
    let mut buffer = [0_u16; 16];
    let mut bytes = u32::try_from(std::mem::size_of_val(&buffer)).ok()?;
    // SAFETY: a predefined root key; `key` and the value name outlive the
    // call; `buffer` is writable for `bytes` bytes and `bytes` is a live
    // local the call updates.
    let status = unsafe {
        RegGetValueW(
            root,
            &HSTRING::from(key),
            w!("Value"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status.is_err() {
        return None;
    }
    let len = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16(&buffer[..len]).ok()
}
