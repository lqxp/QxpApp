//! Tor connectivity plugin for QxChat.
//!
//! Provides an opt-in "connect over Tor" mode. When enabled, the app starts an
//! embedded Tor client (Arti) exposing a local SOCKS5 port, then routes the
//! WebView's network traffic through it. The WebView does the actual HTTP/WS/
//! WebRTC transport in JS (`fetch`, `new WebSocket`, `RTCPeerConnection`), so
//! proxying happens at the OS WebView layer — NOT through `reqwest` (reqwest is
//! only used by the updater).
//!
//! Modeled on `screen_audio`: a Tauri plugin exposing `#[tauri::command]`s
//! invoked from the frontend as `invoke("plugin:tor|start", …)`, with managed
//! state and a status event streamed back via `app.emit`.
//!
//! # Architecture
//!   UI (SettingsModal) → invoke("plugin:tor|start")
//!        ↓
//!   engine::TorEngine — boots Arti + local SOCKS5 listener (platform-independent)
//!        ↓
//!   platform module — configure the OS WebView proxy to use 127.0.0.1:{port}
//!        ↓
//!   status emitted as "tor:status" (idle → bootstrapping → ready | error)

mod engine;
mod geo;
mod relays;

use std::sync::{Arc, Mutex};

use tauri::{
    plugin::{Builder, TauriPlugin},
    AppHandle, Emitter, Manager, Runtime, State,
};

/// The default SOCKS5 port Tor exposes locally.
const DEFAULT_SOCKS_PORT: u16 = 9050;

/// Lifecycle state of the embedded Tor client.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TorPhase {
    /// Not started.
    Idle,
    /// Arti is downloading its consensus / establishing the network (listener
    /// not bound yet; network not usable).
    Bootstrapping,
    /// Bootstrap finished and the SOCKS listener is accepting connections.
    Ready,
    /// Bootstrap or startup failed.
    Error,
}

impl TorPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            TorPhase::Idle => "idle",
            TorPhase::Bootstrapping => "bootstrapping",
            TorPhase::Ready => "ready",
            TorPhase::Error => "error",
        }
    }
}

/// Serializable status snapshot returned by `status` and emitted on changes.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TorStatus {
    pub running: bool,
    pub port: u16,
    pub phase: &'static str,
    /// Present only during the `error` phase (bootstrap/startup failure message).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Shared, managed run-state for the Tor client.
pub struct TorState {
    /// The active engine (Arti + SOCKS listener), if any. Dropping it stops Tor.
    engine: Mutex<Option<engine::TorEngine>>,
    port: Mutex<u16>,
    /// Live phase, driven by the engine thread (idle → bootstrapping → ready/error).
    phase: Mutex<TorPhase>,
    /// Last bootstrap/startup error message, cleared on a new start.
    error: Mutex<Option<String>>,
    /// The engine's shared lifecycle stage (phase + latest circuit).
    stage: Mutex<Option<Arc<engine::Stage>>>,
}

impl Default for TorState {
    fn default() -> Self {
        Self {
            engine: Mutex::new(None),
            port: Mutex::new(DEFAULT_SOCKS_PORT),
            phase: Mutex::new(TorPhase::Idle),
            error: Mutex::new(None),
            stage: Mutex::new(None),
        }
    }
}

impl TorState {
    fn is_running(&self) -> bool {
        self.engine.lock().unwrap().is_some()
    }

    /// Public accessor for the tray / app setup to know whether Tor is active.
    pub fn running(&self) -> bool {
        self.is_running()
    }

    fn current_port(&self) -> u16 {
        *self.port.lock().unwrap()
    }

    /// Public accessor so the startup code can read the configured SOCKS port.
    pub fn port(&self) -> u16 {
        self.current_port()
    }

    fn phase(&self) -> TorPhase {
        *self.phase.lock().unwrap()
    }

    fn set_phase(&self, phase: TorPhase) {
        *self.phase.lock().unwrap() = phase;
    }

    fn set_error(&self, msg: Option<String>) {
        *self.error.lock().unwrap() = msg;
    }

    fn as_status(&self) -> TorStatus {
        let phase = self.phase();
        TorStatus {
            running: self.is_running(),
            port: self.current_port(),
            phase: phase.as_str(),
            error: if phase == TorPhase::Error {
                self.error.lock().unwrap().clone()
            } else {
                None
            },
        }
    }
}

fn emit_status<R: Runtime>(app: &AppHandle<R>, state: &TorState) {
    let _ = app.emit("tor:status", state.as_status());
}

/// Returns the current Tor status without side effects.
#[tauri::command]
fn status<R: Runtime>(_app: AppHandle<R>, state: State<'_, TorState>) -> TorStatus {
    state.as_status()
}

/// Starts the embedded Tor client and applies the OS WebView proxy.
#[tauri::command]
async fn start<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, TorState>,
    port: Option<u16>,
) -> Result<TorStatus, String> {
    start_tor(&app, &state, port)
}

/// Starts Tor synchronously (reused by both the `start` command and the tray).
pub fn start_tor<R: Runtime>(
    app: &AppHandle<R>,
    state: &TorState,
    port: Option<u16>,
) -> Result<TorStatus, String> {
    // Idempotent: already running → just report current state.
    if state.is_running() {
        return Ok(state.as_status());
    }

    let requested = port.unwrap_or(DEFAULT_SOCKS_PORT);
    *state.port.lock().unwrap() = requested;
    state.set_error(None);
    state.set_phase(TorPhase::Bootstrapping);
    write_tor_enabled(app, true);

    // Boot Arti + SOCKS listener on a background thread. `bootstrap` blocks the
    // first time (consensus download), so it never runs on the command's
    // runtime; the engine thread flips `stage` to Ready/Error when the listener
    // is actually up (or on failure).
    let stage = Arc::new(engine::Stage::new(TorPhase::Bootstrapping));
    let engine_stage = Arc::clone(&stage);
    let handle = engine::TorEngine::spawn(requested, engine_stage)?;
    *state.engine.lock().unwrap() = Some(handle);
    *state.stage.lock().unwrap() = Some(Arc::clone(&stage));

    // Apply the OS WebView proxy. On platforms where the WebView proxy cannot
    // be applied post-hoc (macOS/WKWebView, WebView2 environment), this is a
    // no-op or returns a descriptive error; the app can still use the raw
    // SOCKS port from the frontend for explicit connections.
    let proxy_result = platform::apply_proxy(app, requested);
    emit_status(app, state);

    if let Err(e) = proxy_result {
        // Report the proxy limitation without tearing down Tor itself.
        eprintln!("[qxchat-tor] webview proxy not applied: {e}");
    }

    // Watch the engine stage from Rust (bootstrap finishes in the background)
    // and stream the transition to `ready` / `error` back to the frontend.
    spawn_phase_watcher(app.clone(), stage);

    Ok(state.as_status())
}

/// Starts Tor synchronously and blocks until the SOCKS listener is ready (or
/// startup fails), so the caller can create the WebView *after* the proxy is
/// actually available. Used only at boot, before the main window opens.
pub fn start_tor_blocking<R: Runtime>(
    app: &AppHandle<R>,
    state: &TorState,
    port: Option<u16>,
    timeout: std::time::Duration,
) -> Result<(), String> {
    if state.is_running() {
        return Ok(());
    }

    let requested = port.unwrap_or(DEFAULT_SOCKS_PORT);
    *state.port.lock().unwrap() = requested;
    state.set_error(None);
    state.set_phase(TorPhase::Bootstrapping);

    let stage = Arc::new(engine::Stage::new(TorPhase::Bootstrapping));
    let engine_stage = Arc::clone(&stage);
    let handle = engine::TorEngine::spawn(requested, engine_stage)?;
    *state.engine.lock().unwrap() = Some(handle);
    *state.stage.lock().unwrap() = Some(Arc::clone(&stage));

    let started = std::time::Instant::now();
    loop {
        match stage.phase() {
            TorPhase::Ready => {
                state.set_phase(TorPhase::Ready);
                let _ = app.emit("tor:status", state.as_status());
                return Ok(());
            }
            TorPhase::Error => {
                state.set_phase(TorPhase::Error);
                state.set_error(stage.error());
                let _ = app.emit("tor:status", state.as_status());
                return Err(state.error.lock().unwrap().clone().unwrap_or_default());
            }
            _ => {}
        }

        if started.elapsed() > timeout {
            state.set_phase(TorPhase::Error);
            state.set_error(Some("Tor bootstrap timed out".into()));
            return Err("Tor bootstrap timed out".into());
        }

        std::thread::sleep(std::time::Duration::from_millis(120));
    }
}

/// Stops Tor and restores direct connectivity.
#[tauri::command]
async fn stop<R: Runtime>(app: AppHandle<R>, state: State<'_, TorState>) -> Result<TorStatus, String> {
    stop_tor(&app, &state)
}

/// Stops Tor synchronously (reused by both the `stop` command and the tray).
pub fn stop_tor<R: Runtime>(app: &AppHandle<R>, state: &TorState) -> Result<TorStatus, String> {
    // Drop the engine (stops Arti + SOCKS listener) and clear the proxy.
    *state.engine.lock().unwrap() = None;
    *state.stage.lock().unwrap() = None;
    state.set_phase(TorPhase::Idle);
    state.set_error(None);
    let _ = platform::clear_proxy(app);
    write_tor_enabled(app, false);
    emit_status(app, state);

    Ok(state.as_status())
}

/// Restores direct connectivity (no proxy).
pub fn clear_proxy<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    platform::clear_proxy(app)
}

/// Applies the OS WebView proxy to route traffic through Tor (runtime, used on
/// Linux where the proxy can be changed post-hoc).
pub fn apply_proxy<R: Runtime>(app: &AppHandle<R>, port: u16) -> Result<(), String> {
    platform::apply_proxy(app, port)
}

/// Watches the engine's `stage` and mirrors it into the managed `TorState`,
/// emitting `tor:status` whenever the phase changes (bootstrapping → ready/error).
fn spawn_phase_watcher<R: Runtime>(app: AppHandle<R>, stage: Arc<engine::Stage>) {
    let _ = std::thread::Builder::new()
        .name("qxchat-tor-status".into())
        .spawn(move || {
            let mut last = stage.phase();
            loop {
                let current = stage.phase();
                if current != last {
                    last = current;

                    // Mirror into the managed state so `status` reflects it too.
                    if let Some(state) = app.try_state::<TorState>() {
                        state.set_phase(current);
                        if current == TorPhase::Error {
                            state.set_error(stage.error());
                        }
                    }

                    let _ = app.emit(
                        "tor:status",
                        app.try_state::<TorState>()
                            .map(|s| s.as_status())
                            .unwrap_or_else(|| TorStatus {
                                running: current == TorPhase::Ready || current == TorPhase::Bootstrapping,
                                port: 0,
                                phase: current.as_str(),
                                error: stage.error(),
                            }),
                    );
                }

                if current == TorPhase::Ready || current == TorPhase::Error {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
        });
}

/// Probes whether the local SOCKS5 port is accepting connections (readiness).
#[tauri::command]
async fn is_ready<R: Runtime>(
    _app: AppHandle<R>,
    state: State<'_, TorState>,
) -> Result<bool, String> {
    let port = state.current_port();
    probe_port(port).await
}

/// Fetches the Tor relay directory through the local Tor SOCKS5 proxy.
///
/// This is deliberately routed through Tor (not the WebView) so that even
/// consulting the public relay directory does not leak a client-side DNS query.
#[tauri::command]
async fn relays<R: Runtime>(
    _app: AppHandle<R>,
    state: State<'_, TorState>,
    limit: Option<usize>,
) -> Result<Vec<relays::TorRelayInfo>, String> {
    // The relay directory is fetched through the local Tor SOCKS5 proxy. That
    // only works once Tor is actually running; otherwise the request would just
    // fail with a connection-refused error, so surface a clear message instead.
    if !state.is_running() {
        return Err("Tor is not running. Start Tor before loading the relay directory.".into());
    }

    let port = state.current_port();
    let limit = limit.unwrap_or(100).clamp(1, 500);
    relays::fetch_relays(port, limit).await
}

/// Returns the most recently established Tor circuit (guard → middle → exit),
/// populated as the SOCKS proxy forwards real traffic.
#[tauri::command]
fn circuit<R: Runtime>(
    _app: AppHandle<R>,
    state: State<'_, TorState>,
) -> Result<Option<engine::CircuitPath>, String> {
    // The circuit is only meaningful while Tor is running.
    if !state.is_running() {
        return Ok(None);
    }
    Ok(state
        .stage
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|s| s.circuit()))
}

/// Fetches the client's and server's coarse geolocation for the Tor map
/// (public IP is masked; only country/lat/lng/AS are returned).
#[tauri::command]
async fn geo<R: Runtime>(_app: AppHandle<R>) -> Result<geo::GeoInfo, String> {
    geo::fetch_geo().await
}

/// Cheap TCP connectivity probe against the local SOCKS listener.
async fn probe_port(port: u16) -> Result<bool, String> {
    match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Filesystem location of the "tor enabled" marker, read at boot so the WebView
/// can be created with the right proxy before the frontend ever loads.
fn tor_marker_path<R: Runtime>(app: &AppHandle<R>) -> Option<std::path::PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    Some(dir.join("tor-enabled"))
}

/// Persists the boot-time Tor preference (used by the startup proxy wiring).
pub fn write_tor_enabled<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    let Some(path) = tor_marker_path(app) else { return };
    // `app_data_dir` may not exist yet on first run; create it before writing.
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, if enabled { b"1" } else { b"0" }) {
        eprintln!("[qxchat-tor] failed to persist tor-enabled: {e}");
    }
}

/// Reads the persisted boot-time Tor preference.
pub fn read_tor_enabled<R: Runtime>(app: &AppHandle<R>) -> bool {
    let Some(path) = tor_marker_path(app) else { return false };
    matches!(std::fs::read(&path), Ok(bytes) if bytes == b"1")
}

/// Initializes the Tor plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("tor")
        .invoke_handler(tauri::generate_handler![status, start, stop, is_ready, relays, circuit, geo])
        .setup(|app, _api| {
            app.manage(TorState::default());
            Ok(())
        })
        .build()
}

/// Per-OS WebView proxy glue.
#[cfg_attr(target_os = "windows", path = "tor/windows.rs")]
#[cfg_attr(target_os = "macos", path = "tor/macos.rs")]
#[cfg_attr(target_os = "linux", path = "tor/linux.rs")]
#[cfg_attr(
    any(
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ),
    path = "tor/linux.rs"
)]
mod platform;
