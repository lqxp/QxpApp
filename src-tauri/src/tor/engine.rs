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
use std::sync::{Arc, Mutex};

use arti_client::{IntoTorAddr, TorClient};
use tor_rtcompat::Runtime;

/// A single hop of the live circuit, resolved to the fields the UI needs to
/// show the Tor-Browser-style circuit display.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CircuitHopInfo {
    /// "guard" | "middle" | "exit".
    pub role: &'static str,
    /// First IPv4/IPv6 socket address of the relay, if known.
    pub ip: Option<String>,
    /// Relay nickname from the consensus (e.g. "Unnamed" style names).
    pub nickname: String,
    /// ISO 3166-1 alpha-2 country code, if known (requires geoip).
    pub country: Option<String>,
    /// Ed25519 identity, base64 (fingerprint).
    pub ed25519: Option<String>,
    /// RSA identity, hex (fingerprint).
    pub rsa: Option<String>,
}

/// The currently-active circuit (guard → middle → exit).
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CircuitPath {
    pub hops: Vec<CircuitHopInfo>,
}

/// Shared lifecycle state driven by the engine thread and observed by `tor.rs`
/// so the frontend can show "bootstrapping" until Tor is actually ready.
pub struct Stage {
    phase: Mutex<super::TorPhase>,
    error: Mutex<Option<String>>,
    /// Most recently established circuit (filled from `serve_socks`).
    circuit: Mutex<Option<CircuitPath>>,
}

impl Stage {
    pub fn new(phase: super::TorPhase) -> Self {
        Self {
            phase: Mutex::new(phase),
            error: Mutex::new(None),
            circuit: Mutex::new(None),
        }
    }

    pub fn phase(&self) -> super::TorPhase {
        *self.phase.lock().unwrap()
    }

    pub fn set_phase(&self, phase: super::TorPhase) {
        *self.phase.lock().unwrap() = phase;
    }

    pub fn set_error(&self, msg: impl Into<String>) {
        *self.error.lock().unwrap() = Some(msg.into());
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    /// Publishes the most recently observed circuit path.
    pub fn set_circuit(&self, path: CircuitPath) {
        *self.circuit.lock().unwrap() = Some(path);
    }

    pub fn circuit(&self) -> Option<CircuitPath> {
        self.circuit.lock().unwrap().clone()
    }
}

/// Shared, managed engine handle. Dropping it stops Tor and the SOCKS listener.
pub struct TorEngine {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TorEngine {
    /// Spawns the engine in a background thread (Arti + SOCKS5 listener).
    ///
    /// `stage` is flipped to `Ready` once the SOCKS listener is actually bound
    /// (i.e. the network is usable), or to `Error` (with a message) on failure.
    pub fn spawn(port: u16, stage: Arc<Stage>) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);

        let thread = std::thread::Builder::new()
            .name("qxchat-tor".into())
            .spawn(move || {
                if let Err(e) = run_engine(thread_stop, port, Arc::clone(&stage)) {
                    eprintln!("[qxchat-tor] engine error: {e}");
                    // Ensure the watcher never spins forever: mark Error even for
                    // failures that happen before `run_engine` reaches its own
                    // phase bookkeeping (e.g. tokio runtime build failure).
                    if stage.phase() != super::TorPhase::Ready {
                        stage.set_phase(super::TorPhase::Error);
                        stage.set_error(e);
                    }
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
fn run_engine(stop: Arc<AtomicBool>, port: u16, stage: Arc<Stage>) -> Result<(), String> {
    // Create a Tokio runtime for Arti + the SOCKS listener. Arti's
    // `PreferredRuntime` is Tokio, so `TorClient::builder()` must run inside a
    // Tokio context.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;

    rt.block_on(async move {
        let client = match TorClient::builder().create_unbootstrapped() {
            Ok(c) => c,
            Err(e) => {
                stage.set_phase(super::TorPhase::Error);
                stage.set_error(format!("arti create: {e}"));
                return Err(format!("arti create: {e}"));
            }
        };

        // Bootstrap to the live network (this is the slow part: first boot
        // downloads the consensus, typically a few seconds).
        if let Err(e) = client.bootstrap().await {
            stage.set_phase(super::TorPhase::Error);
            stage.set_error(format!("arti bootstrap: {e}"));
            return Err(format!("arti bootstrap: {e}"));
        }

        // The SOCKS listener is what makes the proxy usable; only once it is
        // bound do we mark Tor ready.
        stage.set_phase(super::TorPhase::Ready);
        serve_socks(&client, port, stop, stage).await
    })
}

/// Minimal SOCKS5 CONNECT-only proxy (no auth, no UDP associate).
async fn serve_socks<R: Runtime>(
    client: &TorClient<R>,
    port: u16,
    stop: Arc<AtomicBool>,
    stage: Arc<Stage>,
) -> Result<(), String> {
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
        let stage = Arc::clone(&stage);

        tokio::spawn(async move {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let _ = handle_socks_conn(client, &mut socket, stage).await;
        });
    }

    Ok(())
}

/// Handles one SOCKS5 connection: greeting → CONNECT → relay bytes.
async fn handle_socks_conn<R: Runtime>(
    client: TorClient<R>,
    socket: &mut tokio::net::TcpStream,
    stage: Arc<Stage>,
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

    // Publish the circuit this stream is riding on, so the UI can show the
    // live guard → middle → exit path (like Tor Browser). Best-effort: failures
    // here must never break the proxy.
    let _ = publish_circuit(&client, &stream, &stage);

    // Success reply.
    let _ = socket.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;

    // Relay bidirectional traffic. `DataStream` implements both
    // `tokio::io::AsyncRead` and `AsyncWrite`, so it can be copied against the
    // TCP socket directly.
    let _ = tokio::io::copy_bidirectional(&mut stream, socket).await;

    Ok(())
}

/// Resolves the circuit of `stream` into a [`CircuitPath`] and stores it on
/// `stage`. IPs and fingerprints come directly off the circuit's
/// `OwnedChanTarget`s (role is derived from hop position); nicknames and
/// countries come from the live directory. Best-effort: failures must never
/// break the proxy itself.
fn publish_circuit<R: Runtime>(
    client: &TorClient<R>,
    stream: &arti_client::DataStream,
    stage: &Stage,
) -> Result<(), String> {
    use tor_geoip::HasCountryCode as _;
    use tor_linkspec::{HasAddrs as _, HasRelayIds as _, RelayId};

    let hops = stream.circuit().path();
    if hops.len() < 2 {
        return Ok(());
    }

    // Resolve nicknames + countries from the live directory (best-effort).
    let netdir = client.dirmgr().netdir(tor_netdir::Timeliness::Timely).ok();

    let last = hops.len() - 1;
    let mut resolved = Vec::with_capacity(hops.len());
    for (i, hop) in hops.iter().enumerate() {
        let role = if i == 0 {
            "guard"
        } else if i == last {
            "exit"
        } else {
            "middle"
        };

        let ed25519 = hop.ed_identity().map(|id| id.to_string());
        let rsa = hop.rsa_identity().map(|id| id.to_string());
        let mut ip = hop.addrs().first().map(|a| a.ip().to_string());
        let mut nickname = "Unnamed".to_string();
        let mut country = None;

        // Enrich from the directory using the relay's Ed25519 identity.
        if let (Some(nd), Some(ed)) = (&netdir, hop.ed_identity()) {
            let relay_id = RelayId::Ed25519(*ed);
            if let Some(relay) = nd.by_id(&relay_id) {
                nickname = relay.rs().nickname().to_string();
                if ip.is_none() {
                    ip = relay.addrs().first().map(|a| a.ip().to_string());
                }
                country = relay.country_code().map(|c| c.to_string());
            }
        }

        resolved.push(CircuitHopInfo {
            role,
            ip,
            nickname,
            country,
            ed25519,
            rsa,
        });
    }

    stage.set_circuit(CircuitPath { hops: resolved });
    Ok(())
}
