//! Embedded Tor engine: boots Arti and exposes a local SOCKS5 proxy.
//!
//! This is the platform-independent core. It:
//!   1. creates a `TorClient` (Arti) with the default config,
//!   2. bootstraps it to the live Tor network,
//!   3. binds a `TcpListener` on `127.0.0.1:{port}` acting as a minimal SOCKS5
//!      CONNECT proxy whose upstream is `TorClient::connect`, and
//!   4. reports progress through a callback so the UI can show "bootstrapping".
//!
//! The `tor/<os>.rs` modules only configure the OS WebView proxy to point at
//! this local SOCKS5 port; they do not re-implement Tor.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use arti_client::{IntoTorAddr, TorClient};
use tor_rtcompat::Runtime;

/// Shared, managed engine handle. Dropping it stops Tor and the SOCKS listener.
pub struct TorEngine {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TorEngine {
    /// Spawns the engine in a background thread (Arti + SOCKS5 listener).
    pub fn spawn(port: u16) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);

        let thread = std::thread::Builder::new()
            .name("qxchat-tor".into())
            .spawn(move || {
                if let Err(e) = run_engine(thread_stop, port) {
                    eprintln!("[qxchat-tor] engine error: {e}");
                }
            })
            .map_err(|e| format!("failed to spawn Tor thread: {e}"))?;

        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for TorEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Runs Arti bootstrap + the SOCKS5 CONNECT loop on the current thread.
fn run_engine(stop: Arc<AtomicBool>, port: u16) -> Result<(), String> {
    // Create a Tokio runtime for Arti + the SOCKS listener. Arti's
    // `PreferredRuntime` is Tokio, so `TorClient::builder()` must run inside a
    // Tokio context.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;

    rt.block_on(async move {
        let client = TorClient::builder()
            .create_unbootstrapped()
            .map_err(|e| format!("arti create: {e}"))?;

        // Bootstrap to the live network (this is the slow part: first boot
        // downloads the consensus, typically a few seconds).
        client
            .bootstrap()
            .await
            .map_err(|e| format!("arti bootstrap: {e}"))?;

        serve_socks(&client, port, stop).await
    })
}

/// Minimal SOCKS5 CONNECT-only proxy (no auth, no UDP associate).
async fn serve_socks<R: Runtime>(
    client: &TorClient<R>,
    port: u16,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| format!("socks bind 127.0.0.1:{port}: {e}"))?;

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }

        let (mut socket, _peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(_) => continue,
        };

        let client = TorClient::clone(client);
        let stop = Arc::clone(&stop);

        tokio::spawn(async move {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let _ = handle_socks_conn(client, &mut socket).await;
        });
    }

    Ok(())
}

/// Handles one SOCKS5 connection: greeting → CONNECT → relay bytes.
async fn handle_socks_conn<R: Runtime>(
    client: TorClient<R>,
    socket: &mut tokio::net::TcpStream,
) -> Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // --- Greeting: read methods offered by the client. ---
    let mut header = [0u8; 2];
    socket.read_exact(&mut header).await.map_err(|_| "read greeting")?;
    if header[0] != 0x05 {
        return Err("not socks5".into());
    }
    let nmethods = header[1] as usize;
    let mut methods = vec![0u8; nmethods];
    socket.read_exact(&mut methods).await.map_err(|_| "read methods")?;

    // Reply "no authentication required" (method 0x00).
    socket
        .write_all(&[0x05, 0x00])
        .await
        .map_err(|_| "write method")?;

    // --- Request: version, cmd, addr. ---
    let mut req = [0u8; 4];
    socket.read_exact(&mut req).await.map_err(|_| "read request")?;
    if req[0] != 0x05 || req[1] != 0x01 {
        // Only CONNECT is supported.
        let _ = socket.write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
        return Err("unsupported command".into());
    }

    // Parse the destination address.
    let (host, port) = match req[3] {
        0x01 => {
            // IPv4
            let mut ip = [0u8; 4];
            socket.read_exact(&mut ip).await.map_err(|_| "read ipv4")?;
            let mut p = [0u8; 2];
            socket.read_exact(&mut p).await.map_err(|_| "read ipv4 port")?;
            let p = u16::from_be_bytes(p);
            (std::net::IpAddr::V4(ip.into()).to_string(), p)
        }
        0x03 => {
            // Domain name
            let mut len = [0u8; 1];
            socket.read_exact(&mut len).await.map_err(|_| "read domain len")?;
            let mut domain = vec![0u8; len[0] as usize];
            socket.read_exact(&mut domain).await.map_err(|_| "read domain")?;
            let mut p = [0u8; 2];
            socket.read_exact(&mut p).await.map_err(|_| "read domain port")?;
            let p = u16::from_be_bytes(p);
            (String::from_utf8_lossy(&domain).to_string(), p)
        }
        _ => {
            // Only IPv4 and domain are handled for the common Tor case.
            let _ = socket.write_all(&[0x05, 0x08, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
            return Err("unsupported address type".into());
        }
    };

    // Connect through Tor. Arti resolves the host (and supports `.onion`).
    let addr = (host.as_str(), port)
        .into_tor_addr()
        .map_err(|e| format!("tor addr: {e}"))?;

    let mut stream = match client.connect(addr).await {
        Ok(s) => s,
        Err(_) => {
            // General failure (0x05 = connection refused/general error).
            let _ = socket.write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
            return Err("tor connect failed".into());
        }
    };

    // Success reply.
    let _ = socket.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;

    // Relay bidirectional traffic. `DataStream` implements both
    // `tokio::io::AsyncRead` and `AsyncWrite`, so it can be copied against the
    // TCP socket directly.
    let _ = tokio::io::copy_bidirectional(&mut stream, socket).await;

    Ok(())
}
