//! This user's registry through `reg.exe` (start at login, the browser
//! connector), so the app keeps no `unsafe` of its own.

use std::os::windows::process::CommandExt as _;
use std::process::{Command, Stdio};

/// `CREATE_NO_WINDOW`: `reg.exe` must not flash a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Run `reg.exe` with `args`; its output when it succeeded.
pub(crate) fn reg(args: &[&str]) -> Option<String> {
    let output = Command::new("reg.exe")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The text of the first `REG_SZ` value in `reg query` output.
pub(crate) fn string_value(query: &str) -> Option<String> {
    query.lines().find_map(|line| {
        let (_, data) = line.split_once("REG_SZ")?;
        Some(data.trim().to_owned())
    })
}
