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
mod relays;

use std::sync::Mutex;

use tauri::{
    plugin::{Builder, TauriPlugin},
    AppHandle, Emitter, Manager, Runtime, State,
};

/// The default SOCKS5 port Tor exposes locally.
const DEFAULT_SOCKS_PORT: u16 = 9050;

/// Serializable status snapshot returned by `status` and emitted on changes.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TorStatus {
    pub running: bool,
    pub port: u16,
    pub phase: &'static str,
}

/// Shared, managed run-state for the Tor client.
pub struct TorState {
    /// The active engine (Arti + SOCKS listener), if any. Dropping it stops Tor.
    engine: Mutex<Option<engine::TorEngine>>,
    port: Mutex<u16>,
}

impl Default for TorState {
    fn default() -> Self {
        Self {
            engine: Mutex::new(None),
            port: Mutex::new(DEFAULT_SOCKS_PORT),
        }
    }
}

impl TorState {
    fn is_running(&self) -> bool {
        self.engine.lock().unwrap().is_some()
    }

    fn current_port(&self) -> u16 {
        *self.port.lock().unwrap()
    }

    fn as_status(&self) -> TorStatus {
        TorStatus {
            running: self.is_running(),
            port: self.current_port(),
            phase: if self.is_running() { "ready" } else { "idle" },
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
    // Idempotent: already running → just report current state.
    if state.is_running() {
        return Ok(state.as_status());
    }

    let requested = port.unwrap_or(DEFAULT_SOCKS_PORT);
    *state.port.lock().unwrap() = requested;

    // Boot Arti + SOCKS listener on a background thread. `bootstrap` blocks the
    // first time (consensus download), so it never runs on the command's
    // runtime; status reaches the UI as soon as the listener is up (or on error).
    let handle = engine::TorEngine::spawn(requested)?;
    *state.engine.lock().unwrap() = Some(handle);

    // Apply the OS WebView proxy. On platforms where the WebView proxy cannot
    // be applied post-hoc (macOS/WKWebView, WebView2 environment), this is a
    // no-op or returns a descriptive error; the app can still use the raw
    // SOCKS port from the frontend for explicit connections.
    let proxy_result = platform::apply_proxy(&app, requested);
    emit_status(&app, &state);

    if let Err(e) = proxy_result {
        // Report the proxy limitation without tearing down Tor itself.
        eprintln!("[qxchat-tor] webview proxy not applied: {e}");
    }

    Ok(state.as_status())
}

/// Stops Tor and restores direct connectivity.
#[tauri::command]
async fn stop<R: Runtime>(app: AppHandle<R>, state: State<'_, TorState>) -> Result<TorStatus, String> {
    // Drop the engine (stops Arti + SOCKS listener) and clear the proxy.
    *state.engine.lock().unwrap() = None;
    let _ = platform::clear_proxy(&app);
    emit_status(&app, &state);

    Ok(state.as_status())
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

/// Cheap TCP connectivity probe against the local SOCKS listener.
async fn probe_port(port: u16) -> Result<bool, String> {
    match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Initializes the Tor plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("tor")
        .invoke_handler(tauri::generate_handler![status, start, stop, is_ready, relays])
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
