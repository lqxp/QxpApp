//! Linux (WebKitGTK) WebView proxy glue.
//!
//! WebKitGTK can change its proxy at runtime through the default `WebContext`,
//! so unlike Windows this works post-hoc. We swap the network proxy settings to
//! route all WebView traffic through the local Tor SOCKS5 listener, and restore
//! `NoProxy` when Tor is stopped.

use tauri::{AppHandle, Manager, Runtime};

/// Points the WebView's default network session at `127.0.0.1:{port}` (SOCKS5).
pub fn apply_proxy<R: Runtime>(app: &AppHandle<R>, port: u16) -> Result<(), String> {
    use webkit2gtk::{
        NetworkProxyMode, NetworkProxySettings, WebContextExtManual, WebViewExt,
    };

    let Some(window) = app.get_webview_window("main") else {
        return Err("main window not found".into());
    };

    let proxy_uri = format!("socks5://127.0.0.1:{port}");

    window
        .with_webview(|webview| {
            let inner = webview.inner();
            let Some(context) = inner.web_context() else {
                return Err("web context unavailable".to_string());
            };

            // Route everything (including localhost-hosted app assets) through
            // Tor, except the local app origin itself which is served by the
            // custom protocol (not subject to the proxy).
            let mut settings = NetworkProxySettings::new(Some(&proxy_uri), &[]);
            // `socks5` is a global default; no per-scheme override needed.
            context.set_network_proxy_settings(
                NetworkProxyMode::Custom,
                Some(&mut settings),
            );
            Ok(())
        })
        .map_err(|e| e.to_string())
}

/// Restores direct connectivity (no proxy).
pub fn clear_proxy<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    use webkit2gtk::{NetworkProxyMode, WebContextExtManual, WebViewExt};

    let Some(window) = app.get_webview_window("main") else {
        return Err("main window not found".into());
    };

    window
        .with_webview(|webview| {
            let inner = webview.inner();
            let Some(context) = inner.web_context() else {
                return Err("web context unavailable".to_string());
            };
            context.set_network_proxy_settings(NetworkProxyMode::NoProxy, None);
            Ok(())
        })
        .map_err(|e| e.to_string())
}
