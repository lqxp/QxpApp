//! Native permission gateway for the QxChat client.
//!
//! On Android the WebView does not reliably surface system permission prompts
//! (camera, microphone, notifications, media/storage). This plugin bridges the
//! frontend to the native Android runtime-permission API through a small Kotlin
//! plugin (`com.qxp.client.PermissionsPlugin`) backed by the `@TauriPlugin`
//! declarations in `PermissionsPlugin.kt`.
//!
//! On desktop / iOS the commands are harmless no-ops that report the relevant
//! permission as "granted" so the frontend can simply continue.

use std::collections::HashMap;

#[cfg(target_os = "android")]
use serde::Deserialize;

#[cfg(target_os = "android")]
use serde_json::Value;

#[cfg(target_os = "android")]
use tauri::plugin::PluginHandle;

use tauri::{
    plugin::{Builder, TauriPlugin},
    Runtime,
};

#[cfg(target_os = "android")]
use tauri::Manager;

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "com.qxp.client";

#[cfg(target_os = "android")]
const PLUGIN_CLASS: &str = "PermissionsPlugin";

/// Flat mapping of permission alias -> "granted" | "denied" | "prompt".
pub type PermissionState = HashMap<String, String>;

/// Native response shape from `checkPermissions` / `requestPermissions`. The
/// Kotlin `Plugin` base class resolves a JSON object keyed by permission alias,
/// so we simply deserialize the flattened object.
#[cfg(target_os = "android")]
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PermissionStatuses {
    #[serde(flatten)]
    statuses: HashMap<String, Value>,
}

#[cfg(target_os = "android")]
impl PermissionStatuses {
    fn into_state(self) -> PermissionState {
        self.statuses
            .into_iter()
            .map(|(key, value)| {
                let state = match &value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (key, state)
            })
            .collect()
    }
}

#[cfg(not(target_os = "android"))]
fn granted(keys: &[&str]) -> PermissionState {
    keys.iter()
        .map(|k| ((*k).to_string(), "granted".to_string()))
        .collect()
}

/// Request every declared permission (camera / microphone / notifications /
/// media). Safe to call repeatedly — already-granted permissions are reported
/// back as granted without re-prompting.
#[tauri::command]
async fn request_permissions<R: Runtime>(app: tauri::AppHandle<R>) -> Result<PermissionState, String> {
    #[cfg(target_os = "android")]
    {
        let handle = app
            .try_state::<PluginHandle<R>>()
            .map(|h| h.inner().clone())
            .ok_or_else(|| "permissions plugin is not registered".to_string())?;

        let statuses: PermissionStatuses = handle
            .run_mobile_plugin_async("requestPermissions", Value::Null)
            .await
            .map_err(|e| e.to_string())?;

        Ok(statuses.into_state())
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(granted(&["camera", "microphone", "notifications", "storage"]))
    }
}

/// Report the current state of every declared permission without prompting.
#[tauri::command]
async fn check_permissions<R: Runtime>(app: tauri::AppHandle<R>) -> Result<PermissionState, String> {
    #[cfg(target_os = "android")]
    {
        let handle = app
            .try_state::<PluginHandle<R>>()
            .map(|h| h.inner().clone())
            .ok_or_else(|| "permissions plugin is not registered".to_string())?;

        let statuses: PermissionStatuses = handle
            .run_mobile_plugin_async("checkPermissions", Value::Null)
            .await
            .map_err(|e| e.to_string())?;

        Ok(statuses.into_state())
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(granted(&["camera", "microphone", "notifications", "storage"]))
    }
}

/// Initializes the permissions plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("permissions")
        .invoke_handler(tauri::generate_handler![
            request_permissions,
            check_permissions
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
