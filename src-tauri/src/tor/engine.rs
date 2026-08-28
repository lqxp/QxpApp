//! Embedded Tor engine + a permanent local SOCKS5 proxy.
//!
//! The proxy listens on `127.0.0.1:{port}` for the whole application lifetime,
//! so the WebView can be configured once with `--proxy-server=socks5://…` and
//! never hit a dead port. Each connection is routed through either:
//!   * `Direct` — plain TCP (passthrough), used when Tor is disabled, or
//!   * `Tor`    — an embedded Arti `TorClient`, used when Tor is enabled.
//!
//! The `tor/<os>.rs` modules only configure the OS WebView proxy to point at
//! this local SOCKS5 port; they do not re-implement anything.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use arti_client::{IntoTorAddr, TorClient};
use tor_rtcompat::{BlockOn, PreferredRuntime};

/// A single hop of the live circuit, resolved to the fields the UI needs to
/// show the Tor-Browser-style circuit display.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CircuitHopInfo {
    pub role: &'static str,
    pub ip: Option<String>,
    pub nickname: String,
    pub country: Option<String>,
    pub ed25519: Option<String>,
    pub rsa: Option<String>,
}

/// The currently-active circuit (guard → middle → exit).
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CircuitPath {
    pub hops: Vec<CircuitHopInfo>,
}

/// Shared lifecycle state driven by the proxy thread and observed by `tor.rs`.
pub struct Stage {
    phase: Mutex<super::TorPhase>,
    error: Mutex<Option<String>>,
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

    /// Sets (or clears, with `None`) the current error message.
    pub fn set_error(&self, msg: Option<impl Into<String>>) {
        *self.error.lock().unwrap() = msg.map(Into::into);
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    pub fn set_circuit(&self, path: CircuitPath) {
        *self.circuit.lock().unwrap() = Some(path);
    }

    pub fn circuit(&self) -> Option<CircuitPath> {
        self.circuit.lock().unwrap().clone()
    }
}

/// What the permanent proxy routes each connection through.
#[derive(Clone)]
enum ProxyMode {
    Direct,
    Tor(Arc<TorClient<PreferredRuntime>>),
}

/// Shared, managed engine handle. It owns the always-on SOCKS5 proxy thread;
/// dropping it stops the proxy.
pub struct TorEngine {
    stop: Arc<AtomicBool>,
    mode: Arc<Mutex<ProxyMode>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TorEngine {
    /// Starts the permanent SOCKS5 proxy on `127.0.0.1:{port}` in `Direct` mode.
    pub fn spawn(port: u16, stage: Arc<Stage>) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let mode = Arc::new(Mutex::new(ProxyMode::Direct));
        let thread_stop = Arc::clone(&stop);
        let thread_mode = Arc::clone(&mode);

        let thread = std::thread::Builder::new()
            .name("qxchat-proxy".into())
            .spawn(move || {
                // `run_proxy` is async; give this dedicated thread its own tokio
                // runtime and block on the proxy loop.
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        eprintln!("[qxchat-proxy] failed to build runtime: {e}");
                        stage.set_phase(super::TorPhase::Error);
                        stage.set_error(Some(e.to_string()));
                        return;
                    }
                };

                if let Err(e) = rt.block_on(run_proxy(thread_stop, port, thread_mode, Arc::clone(&stage))) {
                    eprintln!("[qxchat-proxy] engine error: {e}");
                    stage.set_phase(super::TorPhase::Error);
                    stage.set_error(Some(e));
                }
            })
            .map_err(|e| format!("failed to spawn proxy thread: {e}"))?;

        Ok(Self {
            stop,
            mode,
            thread: Some(thread),
        })
    }

    /// Bootstraps Arti on a background thread, then flips the proxy to Tor mode
    /// and updates `stage` to Ready/Error.
    pub fn enable_tor(&self, stage: Arc<Stage>) {
        let mode = Arc::clone(&self.mode);
        std::thread::Builder::new()
            .name("qxchat-tor-bootstrap".into())
            .spawn(move || {
                match bootstrap_tor_client() {
                    Ok(client) => {
                        *mode.lock().unwrap() = ProxyMode::Tor(client);
                        stage.set_phase(super::TorPhase::Ready);
                    }
                    Err(e) => {
                        eprintln!("[qxchat-tor] bootstrap error: {e}");
                        stage.set_phase(super::TorPhase::Error);
                        stage.set_error(Some(e));
                    }
                }
            })
            .map_err(|e| eprintln!("[qxchat-tor] failed to spawn bootstrap thread: {e}"))
            .ok();
    }

    /// Flips the proxy back to direct (passthrough) mode.
    pub fn disable_tor(&self) {
        *self.mode.lock().unwrap() = ProxyMode::Direct;
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

/// Builds and bootstraps a [`TorClient`], returning it wrapped in an `Arc` so
/// it can be shared into the proxy's routing mode. This is the slow part (first
/// boot downloads the consensus), so it runs on its own thread.
pub fn bootstrap_tor_client() -> Result<Arc<TorClient<PreferredRuntime>>, String> {
    // Create a *self-managed* runtime that owns its own tokio executor and keeps
    // it alive for as long as this handle lives. This is critical: if we only
    // wrapped the currently-running tokio runtime (obtained via
    // `TorClient::builder()` / `PreferredRuntime::current()`), that runtime is
    // local to this bootstrap thread's `block_on` and gets dropped when we
    // return — leaving the TorClient with a dead runtime and making every
    // subsequent `connect()` silently fail ("port listening but no network").
    let runtime = PreferredRuntime::create().map_err(|e| format!("tor runtime: {e}"))?;

    // `TorClient::with_runtime` takes ownership of `runtime`, so the executor
    // stays alive for as long as the client (kept in an `Arc` by the caller).
    let client = runtime.clone().block_on(async {
        let client = TorClient::with_runtime(runtime)
            .create_unbootstrapped()
            .map_err(|e| format!("arti create: {e}"))?;

        client
            .bootstrap()
            .await
            .map_err(|e| format!("arti bootstrap: {e}"))?;

        Ok::<_, String>(client)
    })?;

    Ok(Arc::new(client))
}

/// Accepts SOCKS5 connections and routes them by the current [`ProxyMode`].
async fn run_proxy(
    stop: Arc<AtomicBool>,
    port: u16,
    mode: Arc<Mutex<ProxyMode>>,
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

        let stop = Arc::clone(&stop);
        let mode = Arc::clone(&mode);
        let stage = Arc::clone(&stage);

        tokio::spawn(async move {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let _ = handle_socks_conn(mode, &mut socket, stage).await;
        });
    }

    Ok(())
}

/// Handles one SOCKS5 connection: greeting → CONNECT → relay bytes, routing
/// through Tor or directly depending on `mode`.
async fn handle_socks_conn(
    mode: Arc<Mutex<ProxyMode>>,
    socket: &mut tokio::net::TcpStream,
    stage: Arc<Stage>,
) -> Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // --- Greeting ---
    let mut header = [0u8; 2];
    socket.read_exact(&mut header).await.map_err(|_| "read greeting")?;
    if header[0] != 0x05 {
        return Err("not socks5".into());
    }
    let nmethods = header[1] as usize;
    let mut methods = vec![0u8; nmethods];
    socket.read_exact(&mut methods).await.map_err(|_| "read methods")?;

    // Reply "no authentication required" (method 0x00).
    socket.write_all(&[0x05, 0x00]).await.map_err(|_| "write method")?;

    // --- Request: version, cmd, addr ---
    let mut req = [0u8; 4];
    socket.read_exact(&mut req).await.map_err(|_| "read request")?;
    if req[0] != 0x05 || req[1] != 0x01 {
        let _ = socket.write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
        return Err("unsupported command".into());
    }

    // Parse the destination address.
    let (host, port) = match req[3] {
        0x01 => {
            let mut ip = [0u8; 4];
            socket.read_exact(&mut ip).await.map_err(|_| "read ipv4")?;
            let mut p = [0u8; 2];
            socket.read_exact(&mut p).await.map_err(|_| "read ipv4 port")?;
            (std::net::IpAddr::V4(ip.into()).to_string(), u16::from_be_bytes(p))
        }
        0x03 => {
            let mut len = [0u8; 1];
            socket.read_exact(&mut len).await.map_err(|_| "read domain len")?;
            let mut domain = vec![0u8; len[0] as usize];
            socket.read_exact(&mut domain).await.map_err(|_| "read domain")?;
            let mut p = [0u8; 2];
            socket.read_exact(&mut p).await.map_err(|_| "read domain port")?;
            (String::from_utf8_lossy(&domain).to_string(), u16::from_be_bytes(p))
        }
        _ => {
            let _ = socket.write_all(&[0x05, 0x08, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
            return Err("unsupported address type".into());
        }
    };

    // Route according to the current mode (clone the Arc'd client so the lock
    // is not held across any await point).
    let mode_snapshot = { mode.lock().unwrap().clone() };
    match mode_snapshot {
        ProxyMode::Direct => {
            let upstream = tokio::net::TcpStream::connect((host.as_str(), port))
                .await
                .map_err(|_| "direct connect failed")?;
            let _ = socket.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
            let mut upstream = upstream;
            let _ = tokio::io::copy_bidirectional(&mut upstream, socket).await;
        }
        ProxyMode::Tor(client) => {
            let addr = (host.as_str(), port)
                .into_tor_addr()
                .map_err(|e| format!("tor addr: {e}"))?;
            let mut stream = match client.connect(addr).await {
                Ok(s) => s,
                Err(_) => {
                    let _ = socket.write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
                    return Err("tor connect failed".into());
                }
            };
            let _ = publish_circuit(&client, &stream, &stage);
            let _ = socket.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
            let _ = tokio::io::copy_bidirectional(&mut stream, socket).await;
        }
    }

    Ok(())
}

/// Resolves the circuit of `stream` into a [`CircuitPath`] and stores it on
/// `stage`. Best-effort.
fn publish_circuit(
    client: &Arc<TorClient<PreferredRuntime>>,
    stream: &arti_client::DataStream,
    stage: &Stage,
) -> Result<(), String> {
    use tor_linkspec::{HasAddrs as _, HasRelayIds as _};

    let path = stream.circuit().path_ref();
    let n = path.n_hops();
    if n < 2 {
        return Ok(());
    }

    let netdir = client.dirmgr().netdir(tor_netdir::Timeliness::Timely).ok();

    let last = n - 1;
    let mut resolved = Vec::with_capacity(n);
    for (i, entry) in path.iter().enumerate() {
        let Some(hop) = entry.as_chan_target() else {
            continue;
        };

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

        if let (Some(nd), Some(ed)) = (&netdir, hop.ed_identity()) {
            let relay_id = tor_linkspec::RelayId::Ed25519(*ed);
            if let Some(relay) = nd.by_id(&relay_id) {
                nickname = relay.rs().nickname().to_string();
                if ip.is_none() {
                    ip = relay.addrs().first().map(|a| a.ip().to_string());
                }
            }
        }

        // Country is deliberately NOT taken from the `tor-geoip` database: the
        // relay directory is frequently stale for guards/exits. The frontend
        // resolves country + exact coordinates for every hop IP through the
        // Rust geo-IP backend instead. `country` is kept as a legacy field
        // (always `None`) so the wire schema stays backward-compatible.
        let country: Option<String> = None;

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
