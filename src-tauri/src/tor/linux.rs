//! Linux (WebKitGTK) WebView proxy glue.
//!
//! WebKitGTK can change its proxy at runtime through the default `WebContext`'s
//! `WebsiteDataManager`, so unlike Windows this works post-hoc. We swap the
//! network proxy settings to route all WebView traffic through the local Tor
//! SOCKS5 listener, and restore `NoProxy` when Tor is stopped.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager, Runtime};

/// Points the WebView's default network session at `127.0.0.1:{port}` (SOCKS5).
pub fn apply_proxy<R: Runtime>(app: &AppHandle<R>, port: u16) -> Result<(), String> {
    use webkit2gtk::{
        NetworkProxyMode, NetworkProxySettings, WebContextExt, WebsiteDataManagerExt, WebViewExt,
    };

    let Some(window) = app.get_webview_window("main") else {
        return Err("main window not found".into());
    };

    let proxy_uri = format!("socks5://127.0.0.1:{port}");
    // `with_webview` requires a `'static` closure, so surface any inner error
    // through an `Arc<Mutex<..>>` slot and move the `proxy_uri` value in.
    let inner_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    window.with_webview({
        let inner_error = Arc::clone(&inner_error);
        move |webview| {
            let Some(context) = webview.inner().web_context() else {
                *inner_error.lock().unwrap() = Some("web context unavailable".to_string());
                return;
            };

            // The network proxy settings live on the context's data manager, not
            // on the `WebContext` itself. Route everything (including localhost-
            // hosted app assets) through Tor, except the local app origin itself
            // which is served by the custom protocol (not subject to the proxy).
            let Some(manager) = context.website_data_manager() else {
                *inner_error.lock().unwrap() = Some("website data manager unavailable".to_string());
                return;
            };

            let mut settings = NetworkProxySettings::new(Some(&proxy_uri), &[]);
            manager.set_network_proxy_settings(NetworkProxyMode::Custom, Some(&mut settings));
        }
    })
    .map_err(|e| e.to_string())?;

    let result = match inner_error.lock().unwrap().take() {
        Some(e) => Err(e),
        None => Ok(()),
    };
    result
}

/// Restores direct connectivity (no proxy).
pub fn clear_proxy<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    use webkit2gtk::{NetworkProxyMode, WebContextExt, WebsiteDataManagerExt, WebViewExt};

    let Some(window) = app.get_webview_window("main") else {
        return Err("main window not found".into());
    };

    let inner_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    window.with_webview({
        let inner_error = Arc::clone(&inner_error);
        move |webview| {
            let Some(context) = webview.inner().web_context() else {
                *inner_error.lock().unwrap() = Some("web context unavailable".to_string());
                return;
            };
            let Some(manager) = context.website_data_manager() else {
                *inner_error.lock().unwrap() = Some("website data manager unavailable".to_string());
                return;
            };
            manager.set_network_proxy_settings(NetworkProxyMode::NoProxy, None);
        }
    })
    .map_err(|e| e.to_string())?;

    let result = match inner_error.lock().unwrap().take() {
        Some(e) => Err(e),
        None => Ok(()),
    };
    result
}
