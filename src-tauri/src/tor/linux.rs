//! Linux (WebKitGTK) WebView proxy glue.
//!
//! TODO(real implementation):
//! WebKitGTK routes traffic through a `WebKitNetworkSession`/`SoupSession`
//! proxy resolver. This must be attached during WebView setup in `lib.rs`
//! (alongside the existing media/permissions settings). Until wired up, these
//! are safe no-ops, and the frontend can still use the raw SOCKS port directly
//! for explicit connections.

use tauri::{AppHandle, Runtime};

/// Points the WebView at `127.0.0.1:{port}` as its proxy.
pub fn apply_proxy<R: Runtime>(_app: &AppHandle<R>, _port: u16) -> Result<(), String> {
    // TODO: set a GProxyResolver on the WebKitNetworkSession (proxy over SOCKS5).
    //   Gtk/WebKitGTK exposes `WebKitNetworkSession::get_soup_session()` then
    //   `soup_session_add_feature_by_type(GProxyResolver)` with
    //   `socks5://127.0.0.1:{port}`.
    Ok(())
}

/// Restores direct connectivity (no proxy).
pub fn clear_proxy<R: Runtime>(_app: &AppHandle<R>) -> Result<(), String> {
    // TODO: remove the GProxyResolver added in `apply_proxy`.
    Ok(())
}
