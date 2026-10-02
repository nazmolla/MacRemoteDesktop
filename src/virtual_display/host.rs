//! Client side of the `macrdpdisplay` helper (`display-host/macrdpdisplay.m`):
//! one helper process owns one virtual display. Replacing the display means
//! starting a fresh helper; dropping [`HostProcess`] removes the display.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

use super::host_proto::{parse_reply, HostReply};

/// Upper bound for one command. A create can wait up to 5 s for the previous
/// display to leave, 2-6 s for the mode, then up to 5 s for WindowServer to
/// settle, all after queueing behind another host's change.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(45);
/// How long `Drop` waits for the helper to remove its display before killing it.
const QUIT_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct HostProcess {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<String>,
    /// A command timed out or the helper died: a late reply would be read as the
    /// answer to the next command, so this host is never used again.
    broken: bool,
}

impl HostProcess {
    pub(super) fn spawn(name: &str) -> Result<Self> {
        let path = locate().ok_or_else(|| {
            anyhow!(
                "MACRDP_DISPLAY_HOST=1 needs the macrdpdisplay helper, which was not found \
                 (bundled in Contents/Resources, or set MACRDP_DISPLAY_HOST_HELPER)"
            )
        })?;
        let mut child = Command::new(&path)
            .arg(name)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting {}", path.display()))?;
        let stdin = child.stdin.take().context("helper stdin")?;
        let stdout = child.stdout.take().context("helper stdout")?;
        let stderr = child.stderr.take().context("helper stderr")?;
        let pid = child.id();

        let (tx, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                tracing::info!(target: "macrdp::display_host", pid, "{line}");
            }
        });
        tracing::info!(pid, helper = %path.display(), "display host started");
        Ok(Self {
            child,
            stdin,
            replies,
            broken: false,
        })
    }

    pub(super) fn is_broken(&self) -> bool {
        self.broken
    }

    pub(super) fn command(&mut self, line: &str) -> Result<HostReply> {
        if self.broken {
            return Err(anyhow!("display host is no longer usable"));
        }
        self.broken = true;
        let reply = self.command_inner(line)?;
        self.broken = false;
        Ok(reply)
    }

    fn command_inner(&mut self, line: &str) -> Result<HostReply> {
        writeln!(self.stdin, "{line}")
            .and_then(|()| self.stdin.flush())
            .context("writing to the display host")?;
        match self.replies.recv_timeout(COMMAND_TIMEOUT) {
            Ok(reply) => Ok(parse_reply(&reply)),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(anyhow!(
                "display host did not answer `{line}` within {}s",
                COMMAND_TIMEOUT.as_secs()
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(anyhow!("display host exited while handling `{line}`"))
            }
        }
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "quit").and_then(|()| self.stdin.flush());
        let deadline = Instant::now() + QUIT_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => break,
            }
        }
        tracing::warn!(
            pid = self.child.id(),
            "display host did not exit after quit; killing it"
        );
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(super) fn helper_available() -> bool {
    locate().is_some()
}

fn locate() -> Option<PathBuf> {
    if let Some(p) = crate::tunables::var_os("MACRDP_DISPLAY_HOST_HELPER") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok()?;
    if let Some(bundled) = exe
        .parent()
        .map(|macos_dir| macos_dir.join("../Resources/macrdpdisplay"))
    {
        if bundled.is_file() {
            return Some(bundled);
        }
    }
    exe.ancestors()
        .nth(3)
        .map(|root| root.join("target/display-host/macrdpdisplay"))
        .filter(|p| p.is_file())
}
