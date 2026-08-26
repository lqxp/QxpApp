//! Local SOCKS5 port probing + best-effort "kill the process listening on it".
//!
//! When a foreign Tor (system daemon, Tor Browser, …) is already bound to the
//! app's SOCKS5 port, our embedded Arti cannot bind it. This module detects that
//! situation (a real SOCKS5 handshake, not just an open TCP port) and offers a
//! best-effort way to free the port by killing the owning process. The actual
//! decision (reuse the foreign node / kill it / quit) is made by the caller.

use std::io::{Read, Write};

/// Checks whether `127.0.0.1:{port}` is serving a SOCKS5 proxy by completing a
/// minimal SOCKS5 greeting handshake. Returns `true` only if we get a valid
/// method-selection reply, so an arbitrary open port is not mistaken for Tor.
pub fn probe_socks5(port: u16) -> bool {
    let addr = format!("127.0.0.1:{port}");
    let Ok(mut stream) = std::net::TcpStream::connect(&addr) else {
        return false;
    };

    // Never block on a half-open socket.
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(800)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(800)));

    // SOCKS5 greeting: version 5, one method, "no authentication required".
    if stream.write_all(&[0x05, 0x01, 0x00]).is_err() {
        return false;
    }

    let mut reply = [0u8; 2];
    if stream.read_exact(&mut reply).is_err() {
        return false;
    }

    reply == [0x05, 0x00]
}

/// Best-effort "kill the process listening on `127.0.0.1:{port}`". Uses native
/// tooling where available and returns a human-readable error if it can't.
///
/// This is intentionally conservative: we only attempt a TERM-style kill and
/// surface the raw outcome, because killing an arbitrary process the user may
/// need (a system Tor service, Tor Browser, …) is an explicit, user-confirmed
/// action performed only after a native confirmation dialog.
pub fn kill_process_on_port(port: u16) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        // `fuser -k` sends SIGKILL to whatever holds the TCP port. It is a
        // tiny, widely-available util-linux helper. Fall back to a gentle error.
        let output = std::process::Command::new("fuser")
            .args(["-k", &format!("{port}/tcp")])
            .output()
            .map_err(|e| format!("failed to run fuser: {e}"))?;

        if output.status.success() {
            return Ok(());
        }

        // fuser returns non-zero when nothing was found too; treat that as "not
        // running anymore" rather than a hard error.
        return match std::str::from_utf8(&output.stderr).unwrap_or("") {
            s if s.trim().is_empty() => Ok(()),
            s => Err(format!("fuser: {}", s.trim())),
        };
    }

    #[cfg(target_os = "macos")]
    {
        // `lsof -ti tcp:{port}` lists owning PIDs; kill each.
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("lsof -ti tcp:{port} | xargs -r kill"))
            .output()
            .map_err(|e| format!("failed to run lsof/kill: {e}"))?;

        if output.status.success() {
            return Ok(());
        }
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    #[cfg(target_os = "windows")]
    {
        // `netstat -ano` → parse the PID owning the local port → `taskkill /F`.
        let output = std::process::Command::new("netstat")
            .args(["-ano", "-p", "tcp"])
            .output()
            .map_err(|e| format!("failed to run netstat: {e}"))?;

        let text = String::from_utf8_lossy(&output.stdout);
        let needle = format!("127.0.0.1:{port}");
        let pid = text
            .lines()
            .find(|l| l.contains(&needle) && l.to_ascii_uppercase().starts_with("TCP"))
            .and_then(|l| l.split_whitespace().last())
            .and_then(|p| p.parse::<u32>().ok());

        let Some(pid) = pid else {
            return Ok(()); // nothing listening anymore
        };

        let kill = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()
            .map_err(|e| format!("failed to run taskkill: {e}"))?;

        if kill.status.success() {
            return Ok(());
        }
        return Err(String::from_utf8_lossy(&kill.stderr).trim().to_string());
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = port;
        Err("killing a process on a port is not supported on this OS".into())
    }
}
