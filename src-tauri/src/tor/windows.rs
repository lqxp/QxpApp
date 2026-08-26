//! Windows (WebView2) WebView proxy glue.
//!
//! TODO(real implementation):
//! WebView2 supports a proxy via `ICoreWebView2EnvironmentOptions::
//! put_AdditionalBrowserArguments("--proxy-server=socks5://127.0.0.1:PORT")`,
//! but that MUST be set before the WebView2 environment is created — so the
//! real work belongs in the window/environment creation flow, not a post-hoc
//! command. Until then these are safe no-ops.

use tauri::{AppHandle, Runtime};

pub fn apply_proxy<R: Runtime>(_app: &AppHandle<R>, _port: u16) -> Result<(), String> {
    Err("WebView2 proxy must be set before environment creation; not applied here.".into())
}

pub fn clear_proxy<R: Runtime>(_app: &AppHandle<R>) -> Result<(), String> {
    Ok(())
}
