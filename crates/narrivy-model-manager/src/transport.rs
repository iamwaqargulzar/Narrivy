//! Pluggable byte transports with bounded timeouts.
//!
//! Tests never touch the network: they install a fake transport behind this
//! trait, or use [`FileTransport`] against `file://` URLs in a temp dir.

use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::{ModelManagerError, Result};

/// Moves remote bytes into a local staging file.
pub trait Transport: Send + Sync {
    /// Fetch `url` into `dest` (creating or truncating it), under `timeout`.
    fn fetch(&self, url: &str, dest: &Path, timeout: Duration) -> Result<()>;
}

/// Offline transport for `file://` URLs and plain absolute paths.
///
/// Used by the corruption-recovery scenario and any offline qualification run;
/// enforces the same scheme policy as the registry (`file://` only).
pub struct FileTransport;

impl Transport for FileTransport {
    fn fetch(&self, url: &str, dest: &Path, timeout: Duration) -> Result<()> {
        let source = url
            .strip_prefix("file://")
            .ok_or_else(|| ModelManagerError::UntrustedSourceUrl(url.to_string()))?;
        let deadline = Instant::now() + timeout;
        let mut input = std::fs::File::open(source).map_err(|e| ModelManagerError::Transport {
            url: url.to_string(),
            message: e.to_string(),
        })?;
        let mut output = std::fs::File::create(dest)?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            if Instant::now() >= deadline {
                return Err(ModelManagerError::Timeout {
                    url: url.to_string(),
                    timeout_secs: timeout.as_secs(),
                });
            }
            let n = input
                .read(&mut buf)
                .map_err(|e| ModelManagerError::Transport {
                    url: url.to_string(),
                    message: e.to_string(),
                })?;
            if n == 0 {
                break;
            }
            std::io::copy(&mut &buf[..n], &mut output)?;
        }
        Ok(())
    }
}

/// Placeholder for the future TLS-enabled HF fetcher. The P5 model manager is
/// validated through the [`Transport`] contract; adding a real HTTPS client
/// (with its TLS dependency) is an engine-integration decision and never runs
/// inside unit tests.
#[derive(Debug)]
pub struct NoNetworkTransport;

impl Transport for NoNetworkTransport {
    fn fetch(&self, url: &str, _dest: &Path, _timeout: Duration) -> Result<()> {
        Err(ModelManagerError::Transport {
            url: url.to_string(),
            message: "no network transport compiled into narrivy-model-manager".to_string(),
        })
    }
}
