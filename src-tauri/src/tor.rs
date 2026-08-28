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
mod port;
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
    /// Where the traffic is actually routed: `embedded` (our Arti client, with a
    /// live circuit we can display), `external` (a foreign Tor already bound to
    /// the port — transport works, circuit display is unavailable), or `none`.
    pub mode: &'static str,
    /// Present only during the `error` phase (bootstrap/startup failure message).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Shared, managed run-state for the Tor client.
pub struct TorState {
    /// The permanent SOCKS5 proxy engine (always running).
    engine: Mutex<Option<engine::TorEngine>>,
    port: Mutex<u16>,
    /// The engine's shared lifecycle stage (phase + latest circuit).
    stage: Mutex<Option<Arc<engine::Stage>>>,
    /// True when we detected a foreign Tor already bound to our SOCKS5 port and
    /// decided to reuse it instead of bootstrapping our own Arti client.
    external: Mutex<bool>,
    /// Short-lived cache for the geo lookup (client + server), so repeatedly
    /// opening the Tor map doesn't hammer the free geo-IP providers and trigger
    /// their rate limits.
    geo_cache: Mutex<Option<(std::time::Instant, geo::GeoInfo)>>,
}

impl Default for TorState {
    fn default() -> Self {
        Self {
            engine: Mutex::new(None),
            port: Mutex::new(DEFAULT_SOCKS_PORT),
            stage: Mutex::new(None),
            external: Mutex::new(false),
            geo_cache: Mutex::new(None),
        }
    }
}

impl TorState {
    fn stage(&self) -> Option<Arc<engine::Stage>> {
        self.stage.lock().unwrap().clone()
    }

    fn phase(&self) -> TorPhase {
        self.stage().map(|s| s.phase()).unwrap_or(TorPhase::Idle)
    }

    fn is_running(&self) -> bool {
        matches!(self.phase(), TorPhase::Ready | TorPhase::Bootstrapping)
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

    fn as_status(&self) -> TorStatus {
        let phase = self.phase();
        let error = self.stage().and_then(|s| s.error());
        let external = *self.external.lock().unwrap();
        let mode = if external {
            "external"
        } else if self.is_running() {
            "embedded"
        } else {
            "none"
        };
        TorStatus {
            running: self.is_running(),
            port: self.current_port(),
            phase: phase.as_str(),
            mode,
            error: if phase == TorPhase::Error {
                error
            } else {
                None
            },
        }
    }

    /// Marks whether we are reusing a foreign Tor (external mode).
    fn set_external(&self, value: bool) {
        *self.external.lock().unwrap() = value;
    }

    fn is_external(&self) -> bool {
        *self.external.lock().unwrap()
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

/// Starts tor synchronously (reused by both the `start` command and the tray).
pub fn start_tor<R: Runtime>(
    app: &AppHandle<R>,
    state: &TorState,
    port: Option<u16>,
) -> Result<TorStatus, String> {
    if state.is_running() {
        return Ok(state.as_status());
    }

    let requested = port.unwrap_or(DEFAULT_SOCKS_PORT);
    *state.port.lock().unwrap() = requested;
    write_tor_enabled(app, true);

    // Set bootstrapping on the stage, then ask the engine to bootstrap Tor and
    // flip to Tor mode; `enable_tor` updates the stage to Ready/Error.
    if let Some(stage) = state.stage() {
        stage.set_error(None::<String>);
        stage.set_phase(TorPhase::Bootstrapping);
    }

    if let Some(engine) = state.engine.lock().unwrap().as_ref() {
        if let Some(stage) = state.stage() {
            engine.enable_tor(Arc::clone(&stage));
            spawn_phase_watcher(app.clone(), stage, requested);
        }
    }

    // Apply the OS WebView proxy (runtime; Linux). On Windows the proxy is set
    // via static browser args at window creation, so this is a no-op there.
    let proxy_result = platform::apply_proxy(app, requested);
    if let Err(e) = proxy_result {
        eprintln!("[qxchat-tor] webview proxy not applied: {e}");
    }

    emit_status(app, state);
    Ok(state.as_status())
}

/// Stops Tor and restores direct connectivity.
#[tauri::command]
async fn stop<R: Runtime>(app: AppHandle<R>, state: State<'_, TorState>) -> Result<TorStatus, String> {
    stop_tor(&app, &state)
}

/// Toggles Tor and then requests an app restart.
///
/// This is the *user-facing* toggle (used by both the Settings UI and the tray),
/// distinct from the boot-time auto-start in `InboxView` (which calls
/// `start`/`stop` directly and must NOT restart — otherwise the app would
/// restart-loop on every boot for a persisted Tor-enabled session).
///
/// The restart is necessary because on Windows (WebView2) and macOS (WKWebView)
/// the WebView proxy is fixed at window/environment creation and cannot be
/// changed at runtime. A clean relaunch is the only reliable way to guarantee
/// the WebView's network traffic actually routes through the (possibly) change
/// proxy state.
#[tauri::command]
async fn toggle<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, TorState>,
    enabled: bool,
    port: Option<u16>,
) -> Result<TorStatus, String> {
    toggle_tor(&app, &state, enabled, port)
}

/// Toggles Tor (start/stop) then triggers a clean app restart so the WebView is
/// recreated with the correct proxy configuration.
pub fn toggle_tor<R: Runtime>(
    app: &AppHandle<R>,
    state: &TorState,
    enabled: bool,
    port: Option<u16>,
) -> Result<TorStatus, String> {
    // Persist + flip the proxy routing mode first.
    if enabled {
        start_tor(app, state, port)?;
    } else {
        stop_tor(app, state)?;
    }

    // Now relaunch so the WebView proxy takes effect. `restart()` diverges
    // (`-> !`): it either restarts the process directly (main thread) or
    // requests exit and blocks until the event loop restarts it (other threads)
    // — both reliably relaunch the app, unlike `request_restart()` which can
    // fail to deliver the exit event from a Tauri command thread. The `!` return
    // coerces to `Result<TorStatus, String>`.
    app.restart()
}

/// Stops Tor synchronously (reused by both the `stop` command and the tray).
pub fn stop_tor<R: Runtime>(app: &AppHandle<R>, state: &TorState) -> Result<TorStatus, String> {
    // Flip the permanent proxy back to direct (passthrough); keep the proxy
    // itself running so the WebView's static proxy never points at a dead port.
    if let Some(engine) = state.engine.lock().unwrap().as_ref() {
        engine.disable_tor();
    }
    if let Some(stage) = state.stage() {
        stage.set_phase(TorPhase::Idle);
        stage.set_error(None::<String>);
    }
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

/// Watches the engine's `stage` and emits `tor:status` whenever the phase
/// changes (bootstrapping → ready/error). The bootstrap thread flips the stage;
/// this watcher surfaces that to the frontend.
fn spawn_phase_watcher<R: Runtime>(app: AppHandle<R>, stage: Arc<engine::Stage>, port: u16) {
    let _ = std::thread::Builder::new()
        .name("qxchat-tor-status".into())
        .spawn(move || {
            // Emit the current phase immediately (handles the case where bootstrap
            // has already completed before this watcher started), then enter the
            // change-detection loop.
            let mut last = stage.phase();
            loop {
                let current = stage.phase();
                if current != last {
                    last = current;
                }

                let _ = app.emit(
                    "tor:status",
                    TorStatus {
                        running: matches!(current, TorPhase::Ready | TorPhase::Bootstrapping),
                        port,
                        phase: current.as_str(),
                        mode: "embedded",
                        error: if current == TorPhase::Error {
                            stage.error()
                        } else {
                            None
                        },
                    },
                );

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
    // The circuit is only meaningful for our *embedded* Tor client. A foreign
    // (external) Tor provides transport but exposes no circuit we can read, so
    // report none and let the UI hide the circuit/map.
    if !state.is_running() || state.is_external() {
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
async fn geo<R: Runtime>(_app: AppHandle<R>, state: State<'_, TorState>) -> Result<geo::GeoInfo, String> {
    const GEO_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);

    // Serve a fresh result instead of re-querying on every open. The lookup is
    // coarse and IPs rarely move within a few minutes.
    {
        let cache = state.geo_cache.lock().unwrap();
        if let Some((at, info)) = cache.as_ref() {
            if at.elapsed() < GEO_CACHE_TTL {
                return Ok(info.clone());
            }
        }
    }

    let info = geo::fetch_geo().await?;
    *state.geo_cache.lock().unwrap() = Some((std::time::Instant::now(), info.clone()));
    Ok(info)
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

/// Persists the boot-time Tor preference (on/off only).
///
/// The SOCKS5 port is intentionally fixed at [`DEFAULT_SOCKS_PORT`]: the WebView
/// proxy is baked into `additionalBrowserArgs` at build time and cannot change at
/// runtime, so a user-configurable port would just leave the WebView pointing at
/// a dead port. Only the on/off state is persisted.
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

/// Starts the permanent SOCKS5 proxy (passthrough by default) and stores the
/// engine + stage on the managed state.
fn spawn_proxy<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<TorState>();
    let port = state.current_port();
    let stage = Arc::new(engine::Stage::new(TorPhase::Idle));
    match engine::TorEngine::spawn(port, Arc::clone(&stage)) {
        Ok(handle) => {
            *state.engine.lock().unwrap() = Some(handle);
            *state.stage.lock().unwrap() = Some(stage);
        }
        Err(e) => {
            eprintln!("[qxchat-tor] failed to start proxy: {e}");
        }
    }
}

/// Called when a foreign Tor (or other SOCKS5 proxy) is already bound to our
/// port. Asks the user what to do via a native dialog, then applies the choice.
fn resolve_foreign_tor<R: Runtime>(app: &AppHandle<R>) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

    let handle = app.clone();
    let msg = concat!(
        "Another Tor process is already using port 9050.\n\n",
        "• Use it — route traffic through the existing Tor (without the live circuit view)\n",
        "• Kill it — stop that process and start QxChat's own embedded Tor\n",
        "• Quit — close QxChat",
    );

    handle
        .dialog()
        .message(msg)
        .title("Tor port conflict")
        .buttons(MessageDialogButtons::YesNoCancelCustom(
            "Use the node".to_string(),
            "Kill the node".to_string(),
            "Quit QxChat".to_string(),
        ))
        .show_with_result(move |result| {
            match result {
                tauri_plugin_dialog::MessageDialogResult::Yes => {
                    // Reuse the foreign SOCKS5 proxy for transport; mark external.
                    let state = handle.state::<TorState>();
                    state.set_external(true);
                    // Ensure a stage exists (no proxy engine in external mode) and
                    // reflect a "ready" transport state.
                    let stage = state.stage().unwrap_or_else(|| {
                        let s = Arc::new(engine::Stage::new(TorPhase::Ready));
                        *state.stage.lock().unwrap() = Some(Arc::clone(&s));
                        s
                    });
                    stage.set_phase(TorPhase::Ready);
                    // Persist so a relaunch keeps routing through the port.
                    write_tor_enabled(&handle, true);
                    emit_status(&handle, &state);
                }
                tauri_plugin_dialog::MessageDialogResult::No => {
                    // Kill the foreign process, then start our own proxy + Tor.
                    let port = DEFAULT_SOCKS_PORT;
                    let state = handle.state::<TorState>();
                    match port::kill_process_on_port(port) {
                        Ok(()) => {
                            spawn_proxy(&handle);
                            if read_tor_enabled(&handle) {
                                let st = handle.state::<TorState>();
                                let _ = start_tor(&handle, &st, None);
                            }
                        }
                        Err(e) => {
                            eprintln!("[qxchat-tor] failed to free port {port}: {e}");
                            // Leave external mode so the transport still works via
                            // the (still-present) foreign proxy rather than dying.
                            state.set_external(true);
                            let stage = state.stage().unwrap_or_else(|| {
                                let s = Arc::new(engine::Stage::new(TorPhase::Ready));
                                *state.stage.lock().unwrap() = Some(Arc::clone(&s));
                                s
                            });
                            stage.set_phase(TorPhase::Ready);
                            emit_status(&handle, &state);
                        }
                    }
                }
                _ => {
                    // Cancel/Quit: close the app.
                    handle.exit(0);
                }
            }
        });
}

/// Initializes the Tor plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("tor")
        .invoke_handler(tauri::generate_handler![status, start, stop, toggle, is_ready, relays, circuit, geo])
        .setup(|app, _api| {
            app.manage(TorState::default());

            let port = DEFAULT_SOCKS_PORT;
            // If a foreign Tor already holds our SOCKS port, we cannot bind our
            // own permanent proxy — ask the user what to do instead.
            if port::probe_socks5(port) {
                resolve_foreign_tor(app);
                return Ok(());
            }

            // No conflict: start our permanent proxy, then bootstrap Tor if the
            // user previously left it enabled.
            spawn_proxy(app);

            if read_tor_enabled(app) {
                let state = app.state::<TorState>();
                if let Err(e) = start_tor(app, &state, None) {
                    eprintln!("[qxchat-tor] boot auto-start failed: {e}");
                }
            }

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
