//! P4 audio-policy helper (VER-FUN-007 / VER-REL-005 / VER-REL-009 driver).
//!
//! Thin CLI around `audio_policy` so the qualification runner exercises the
//! real Rust snapshot -> mutate -> journal -> restore path. Prints one JSON
//! line on stdout. Volume values and stable identifiers only — no content.
//!
//! Subcommands:
//!   snapshot                          print current default-sink snapshot
//!   apply <leave|duck> --journal PATH apply policy at session start
//!   restore --journal PATH            clean-exit restore of own entries
//!   recover-dead --journal PATH       REL-009 dead-owner recovery scan
//!   duck-and-die --journal PATH       duck, journal, then SIGKILL own process
//!   session <leave|duck> --journal PATH [--hold-ms N]
//!                                     apply, hold an active session window,
//!                                     then clean-exit restore_own (REL-005);
//!                                     emits one progress JSON per stage

use narrivy_platform_linux::audio_policy::{
    PolicyEngine, StdWpctlRunner, WpctlRunner, parse_wpctl_volume,
};
use narrivy_platform_linux::capture::AudioPolicyJournal;

#[derive(serde::Serialize)]
struct Out {
    ok: bool,
    op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    volume: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    muted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
}

fn emit(out: &Out) -> ! {
    println!("{}", serde_json::to_string(out).unwrap_or_default());
    std::process::exit(if out.ok { 0 } else { 1 })
}

fn err(op: &str, msg: String) -> ! {
    emit(&Out {
        ok: false,
        op: op.to_string(),
        error: Some(msg),
        volume: None,
        muted: None,
        restored: None,
        pid: None,
    })
}

fn opt_value(flag: &str, args: &mut impl Iterator<Item = String>) -> Option<String> {
    loop {
        match args.next() {
            Some(a) if a == flag => return args.next(),
            Some(_) => continue,
            None => return None,
        }
    }
}

fn main() {
    let runner = StdWpctlRunner::default();
    let mut args = std::env::args().skip(1);
    let op = args.next().unwrap_or_default();

    match op.as_str() {
        "snapshot" => match WpctlRunner::get_volume(&runner) {
            Ok(raw) => match parse_wpctl_volume(&raw) {
                Ok(s) => emit(&Out {
                    ok: true,
                    op: op.clone(),
                    error: None,
                    volume: Some(s.volume),
                    muted: Some(s.muted),
                    restored: None,
                    pid: None,
                }),
                Err(e) => err("snapshot", e.to_string()),
            },
            Err(e) => err("snapshot", e.to_string()),
        },
        "apply" | "duck-and-die" => {
            let engine = if op == "duck-and-die" {
                PolicyEngine::Duck
            } else {
                match args.next().as_deref() {
                    Some("leave") => PolicyEngine::Leave,
                    Some("duck") => PolicyEngine::Duck,
                    _ => err("apply", "usage: apply <leave|duck>".into()),
                }
            };
            let path = match opt_value("--journal", &mut args) {
                Some(p) => p,
                None => err("apply", "missing --journal PATH".into()),
            };
            let journal = AudioPolicyJournal::new(path);
            match engine.apply(&runner, &journal) {
                Ok(applied) => {
                    let out = Out {
                        ok: true,
                        op: op.clone(),
                        error: None,
                        volume: applied.prior.as_ref().map(|p| p.volume),
                        muted: applied.prior.as_ref().map(|p| p.muted),
                        restored: None,
                        pid: Some(std::process::id()),
                    };
                    if op == "duck-and-die" {
                        // Abrupt death AFTER the journal write: simulates a
                        // forced kill between mutation and cleanup (REL-009).
                        println!("{}", serde_json::to_string(&out).unwrap_or_default());
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                        unsafe_in_free_helper();
                    }
                    emit(&Out {
                        ok: true,
                        op,
                        error: None,
                        volume: out.volume,
                        muted: out.muted,
                        restored: None,
                        pid: out.pid,
                    })
                }
                Err(e) => err("apply", e.to_string()),
            }
        }
        "session" => {
            let engine = match args.next().as_deref() {
                Some("leave") => PolicyEngine::Leave,
                Some("duck") => PolicyEngine::Duck,
                _ => err("session", "usage: session <leave|duck>".into()),
            };
            let path = match opt_value("--journal", &mut args) {
                Some(p) => p,
                None => err("session", "missing --journal PATH".into()),
            };
            let hold_ms: u64 = opt_value("--hold-ms", &mut args)
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000);
            let journal = AudioPolicyJournal::new(&path);

            // Stage 1: apply.
            let applied = match engine.apply(&runner, &journal) {
                Ok(a) => a,
                Err(e) => err("session", e.to_string()),
            };
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "stage": "applied",
                    "engine": match engine { PolicyEngine::Leave => "leave", PolicyEngine::Duck => "duck" },
                    "prior_volume": applied.prior.as_ref().map(|p| p.volume),
                    "pid": std::process::id(),
                }))
                .unwrap_or_default()
            );
            use std::io::Write as _;
            let _ = std::io::stdout().flush();

            // Stage 2: hold the active-session window.
            if hold_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(hold_ms));
            }

            // Stage 3: clean-exit restoration of OWN entries only.
            let restored = match engine.restore_own(&runner, &journal) {
                Ok(n) => n,
                Err(e) => err("session", e.to_string()),
            };
            let (_, raw_after) = {
                let out = WpctlRunner::get_volume(&runner).unwrap_or_default();
                ((), out)
            };
            let after = parse_wpctl_volume(&raw_after).ok();
            emit(&Out {
                ok: true,
                op,
                error: None,
                volume: after.as_ref().map(|a| a.volume),
                muted: after.as_ref().map(|a| a.muted),
                restored: Some(restored),
                pid: Some(std::process::id()),
            })
        }
        "restore" => {
            let path = match opt_value("--journal", &mut args) {
                Some(p) => p,
                None => err("restore", "missing --journal PATH".into()),
            };
            let n = match PolicyEngine::Duck.restore_own(&runner, &AudioPolicyJournal::new(path)) {
                Ok(n) => n,
                Err(e) => err("restore", e.to_string()),
            };
            emit(&Out {
                ok: true,
                op,
                error: None,
                volume: None,
                muted: None,
                restored: Some(n),
                pid: None,
            })
        }
        "recover-dead" => {
            let path = match opt_value("--journal", &mut args) {
                Some(p) => p,
                None => err("recover-dead", "missing --journal PATH".into()),
            };
            let n = match PolicyEngine::restore_dead_owner_entries(
                &runner,
                &AudioPolicyJournal::new(path),
            ) {
                Ok(n) => n,
                Err(e) => err("recover-dead", e.to_string()),
            };
            emit(&Out {
                ok: true,
                op,
                error: None,
                volume: None,
                muted: None,
                restored: Some(n),
                pid: None,
            })
        }
        other => err(other, "unknown subcommand".into()),
    }
}

// The crate forbids unsafe outside its FFI modules; SIGKILL self-termination
// is delegated to a plain child-free helper via libc through the existing
// crate dependency, kept inside this example only.
fn unsafe_in_free_helper() -> ! {
    // std-only abrupt death: abort() raises SIGABRT, not SIGKILL. To die by
    // an uncatchable signal without unsafe FFI here, spawn /bin/kill against
    // our own PID.
    let pid = std::process::id();
    let status = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
    match status {
        Ok(s) if s.success() => {}
        _ => std::process::abort(),
    }
    loop {
        std::thread::park(); // unreachable once the signal lands
    }
}
