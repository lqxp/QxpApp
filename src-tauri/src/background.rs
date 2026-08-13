//! Background keep-alive for the QxChat client.
//!
//! On Android the OS aggressively suspends WebViews and kills background
//! activities, which tears down the frontend WebSocket and long-lived WebRTC
//! calls. To keep the "precious socket" alive we run a native foreground
//! service (`com.qxp.client.ForegroundService`) with a partial wake lock and a
//! persistent notification. That tells Android the app is doing important
//! background work, so it will not kill/suspend the WebView.
//!
//! The frontend WebSocket remains the sole owner of the QXP wire protocol and
//! E2EE state (moving that into Rust would require reimplementing the encrypted
//! message pipeline and WebRTC signaling). This plugin only guards the process
//! so that socket survives in the background.

use tauri::{
    plugin::{Builder, TauriPlugin},
    Runtime,
};

#[cfg(target_os = "android")]
use tauri::{plugin::PluginHandle, Manager};

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "com.qxp.client";

#[cfg(target_os = "android")]
const PLUGIN_CLASS: &str = "BackgroundPlugin";

/// Starts the background keep-alive (foreground service + wake lock).
#[tauri::command]
async fn start_background<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let handle = app
            .try_state::<PluginHandle<R>>()
            .map(|h| h.inner().clone())
            .ok_or_else(|| "background plugin is not registered".to_string())?;
        handle
            .run_mobile_plugin_async::<()>("start", ())
            .await
            .map_err(|e| e.to_string())
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(())
    }
}

/// Stops the background keep-alive.
#[tauri::command]
async fn stop_background<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let handle = app
            .try_state::<PluginHandle<R>>()
            .map(|h| h.inner().clone())
            .ok_or_else(|| "background plugin is not registered".to_string())?;
        handle
            .run_mobile_plugin_async::<()>("stop", ())
            .await
            .map_err(|e| e.to_string())
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(())
    }
}

/// Queries whether the background keep-alive service is currently running.
#[tauri::command]
async fn is_background_running<R: Runtime>(app: tauri::AppHandle<R>) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        let handle = app
            .try_state::<PluginHandle<R>>()
            .map(|h| h.inner().clone())
            .ok_or_else(|| "background plugin is not registered".to_string())?;
        let running: bool = handle
            .run_mobile_plugin_async::<bool>("isRunning", ())
            .await
            .map_err(|e| e.to_string())?;
        Ok(running)
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(false)
    }
}

/// Initializes the background keep-alive plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("background")
        .invoke_handler(tauri::generate_handler![
            start_background,
            stop_background,
            is_background_running
        ])
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, PLUGIN_CLASS)?;
                app.manage(handle);
            }

            #[cfg(not(target_os = "android"))]
            let _ = (app, api);

            Ok(())
        })
        .build()
}
