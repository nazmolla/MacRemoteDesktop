//! Printer redirection: each printer the client redirects becomes a local CUPS
//! queue. The queue sends jobs to a loopback socket; every connection to it is
//! one job, forwarded unchanged to the client's printer over RDPDR. Jobs are
//! PostScript, so the client's printer (or its driver) must accept PostScript.

use std::process::Command;

use ironrdp_server::RdpdrHandle;
use tokio::io::AsyncReadExt;
use tracing::{info, warn};

/// Apple's generic PostScript driver; macOS no longer allows raw queues.
const GENERIC_PPD: &str = "/System/Library/Frameworks/ApplicationServices.framework/Versions/A/Frameworks/PrintCore.framework/Versions/A/Resources/Generic.ppd";

/// Largest job accepted; anything bigger is dropped rather than buffered.
const MAX_JOB: usize = 512 * 1024 * 1024;

#[derive(Debug)]
pub struct Printer {
    queue: String,
    task: tokio::task::JoinHandle<()>,
}

/// A CUPS queue name from the client's printer name: letters, digits, `-`, `_`.
pub fn queue_name(client_name: &str) -> String {
    let clean: String = client_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let clean = clean.trim_matches('_');
    format!("Viga_{}", if clean.is_empty() { "Printer" } else { clean })
}

impl Printer {
    pub fn start(handle: RdpdrHandle, device_id: u32, name: &str) -> anyhow::Result<Self> {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        std_listener.set_nonblocking(true)?;
        let port = std_listener.local_addr()?.port();
        let queue = queue_name(name);
        let out = Command::new("/usr/sbin/lpadmin")
            .args(["-p", &queue, "-E", "-v", &format!("socket://127.0.0.1:{port}"), "-P", GENERIC_PPD])
            .args(["-D", &format!("{name} (remote, via Viga)")])
            .output()?;
        if !out.status.success() {
            anyhow::bail!("lpadmin failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        info!(queue = %queue, device_id, port, "printer: client printer added as a local queue");
        let listener = tokio::net::TcpListener::from_std(std_listener)?;
        let q = queue.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let mut job = Vec::new();
                let read = (&mut sock).take(MAX_JOB as u64 + 1).read_to_end(&mut job).await;
                if read.is_err() || job.len() > MAX_JOB {
                    warn!(queue = %q, "printer: job unreadable or too large, dropped");
                    continue;
                }
                match handle.print_job(device_id, &job).await {
                    Ok(()) => info!(queue = %q, bytes = job.len(), "printer: job sent to the client"),
                    Err(e) => warn!(queue = %q, error = %e, "printer: client rejected the job"),
                }
            }
        });
        Ok(Self { queue, task })
    }
}

impl Drop for Printer {
    fn drop(&mut self) {
        self.task.abort();
        let _ = Command::new("/usr/sbin/lpadmin").args(["-x", &self.queue]).output();
        info!(queue = %self.queue, "printer: queue removed");
    }
}

#[cfg(test)]
mod tests {
    use super::queue_name;

    #[test]
    fn queue_names_are_cups_safe() {
        assert_eq!(queue_name("HP LaserJet 400 (redirected 2)"), "Viga_HP_LaserJet_400__redirected_2");
        assert_eq!(queue_name("   "), "Viga_Printer");
        assert_eq!(queue_name("Brother-HL_2270"), "Viga_Brother-HL_2270");
    }
}
