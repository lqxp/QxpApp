//! macOS (WKWebView) WebView proxy glue.
//!
//! TODO(real implementation):
//! WKWebView exposes no per-app SOCKS proxy. Options are (a) a system-wide
//! proxy via SystemConfiguration (disruptive, affects all apps) or (b) route at
//! the app's own network layer instead of the WebView. Until that decision is
//! made, these are safe no-ops and the frontend can use the raw SOCKS port.

use tauri::{AppHandle, Runtime};

pub fn apply_proxy<R: Runtime>(_app: &AppHandle<R>, _port: u16) -> Result<(), String> {
    Err("WKWebView has no per-app SOCKS proxy; not applied here.".into())
}

pub fn clear_proxy<R: Runtime>(_app: &AppHandle<R>) -> Result<(), String> {
    Ok(())
}
