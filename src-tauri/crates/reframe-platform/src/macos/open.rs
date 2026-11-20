//! Running `/usr/bin/open`, which launches applications and documents through Launch Services
//! exactly like the Finder (the launched application is not a child of Reframe).

use std::ffi::OsString;
use std::io::Read;
use std::process::{ChildStderr, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::{PlatformError, PlatformResult};

const OPEN: &str = "/usr/bin/open";

/// How long to wait for `open`. It normally returns as soon as Launch Services has started the
/// application, but the first launch of a downloaded application can wait on Gatekeeper.
const OPEN_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// At most this much of `open`'s error output is kept.
const MAX_STDERR: u64 = 16 * 1024;

/// Runs `open` with `args` and reports its error message when it fails. `label` names what is
/// being opened in errors. If `open` is still running after [`OPEN_TIMEOUT`] the launch is
/// assumed to be in progress: `open` is left to finish in the background (its outcome is logged)
/// and `Ok` is returned.
pub(super) fn run_open(args: &[OsString], label: &str) -> PlatformResult<()> {
    let mut child = Command::new(OPEN)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| PlatformError::launch_failed(label, format!("could not run {OPEN}: {e}")))?;
    let stderr = child.stderr.take().map(read_in_background);
    let deadline = Instant::now() + OPEN_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                let message = stderr
                    .and_then(|output| output.recv_timeout(Duration::from_millis(500)).ok())
                    .map(|text| text.trim().to_string())
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| format!("{OPEN} exited with {status}"));
                return Err(PlatformError::launch_failed(label, message));
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                let label = label.to_string();
                let waiter = thread::Builder::new()
                    .name("reframe-open-wait".into())
                    .spawn(move || match child.wait() {
                        Ok(status) if !status.success() => {
                            log::warn!("{OPEN} for {label} exited with {status}")
                        }
                        Err(e) => log::warn!("waiting for {OPEN} ({label}) failed: {e}"),
                        Ok(_) => {}
                    });
                if let Err(e) = waiter {
                    log::warn!("could not wait for {OPEN} in the background: {e}");
                }
                return Ok(());
            }
            Err(e) => return Err(PlatformError::launch_failed(label, e.to_string())),
        }
    }
}

/// Collects a pipe's output on a helper thread so a full pipe can never block `open`.
fn read_in_background(pipe: ChildStderr) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name("reframe-open-stderr".into())
        .spawn(move || {
            let mut output = Vec::new();
            let _ = pipe.take(MAX_STDERR).read_to_end(&mut output);
            let _ = sender.send(String::from_utf8_lossy(&output).into_owned());
        });
    if let Err(e) = spawned {
        log::debug!("could not read {OPEN}'s error output: {e}");
    }
    receiver
}
