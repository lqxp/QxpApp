#[cfg(not(any(target_os = "android", target_os = "ios")))]
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Emitter, Listener, Manager,
};

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use tauri_plugin_dialog::DialogExt;

pub mod permissions;
pub mod background;
// Tor (embedded Arti client + SOCKS5 plumbing) is a desktop-only feature: the
// mobile WebViews expose no per-app proxy, and cross-compiling Arti's default
// native-tls backend to Android/iOS is not supported. Gate the module out on
// mobile so the platform-specific proxy glue and Arti dependencies aren't built.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod tor;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod screen_audio;

#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
use webkit2gtk::{
    glib::prelude::ObjectExt, NotificationPermissionRequest, PermissionRequest,
    PermissionRequestExt, SettingsExt, UserMediaPermissionRequest, WebViewExt,
};

#[cfg(not(target_os = "android"))]
fn hide_window(window: &tauri::Window) {
    let _ = window.hide();
}

#[cfg(target_os = "android")]
fn hide_window(_window: &tauri::Window) {
    // Android n'a pas Window::hide()
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn check_updates_from_tray(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    let _ = app.emit("qx:check-updates", ());
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(permissions::init())
        .plugin(background::init());

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let builder = builder
        .plugin(screen_audio::init())
        .plugin(tor::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }));

    builder
        .setup(|app| {
            // The desktop branch below is the only consumer of `app`; on mobile
            // every block is cfg'd out, so keep the parameter marked as used.
            #[cfg(any(target_os = "android", target_os = "ios"))]
            let _ = &app;

            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                // The main window is created from tauri.conf.json (as before),
                // so we no longer create it manually here. Tor is auto-started by
                // the frontend after the window loads (InboxView), and on Linux
                // the WebView proxy is applied at runtime via `tor::apply_proxy`.
                let quit = MenuItem::with_id(app, "quit", "Quit QxChat", true, None::<&str>)?;

                let show = MenuItem::with_id(app, "show", "Open QxChat", true, None::<&str>)?;
                let check_updates = MenuItem::with_id(
                    app,
                    "check_updates",
                    "Check Updates",
                    true,
                    None::<&str>,
                )?;

                let toggle_tor = CheckMenuItem::with_id(
                    app,
                    "toggle_tor",
                    "Connect through Tor",
                    true,
                    tor::read_tor_enabled(app.handle()),
                    None::<&str>,
                )?;

                let menu = Menu::with_items(app, &[&show, &toggle_tor, &check_updates, &quit])?;

                // Clones for the menu-event handler and the status-sync listener.
                let toggle_tor_menu = toggle_tor.clone();
                let toggle_tor_sync = toggle_tor.clone();

                TrayIconBuilder::new()
                    .icon(app.default_window_icon().expect("missing app icon").clone())
                    .menu(&menu)
                    .on_menu_event(move |app, event| match event.id.as_ref() {
                        "quit" => {
                            app.exit(0);
                        }

                        "show" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }

                        "check_updates" => {
                            check_updates_from_tray(app.clone());
                        }

                        "toggle_tor" => {
                            let enable = !app.state::<tor::TorState>().running();
                            let action = if enable { "enabling" } else { "disabling" };
                            let msg = format!(
                                "Changing this setting will restart QxChat completely. Continue {action} Tor?"
                            );

                            // Ask before restarting (native dialog). `show` is
                            // async and runs on the main thread safely, unlike
                            // `blocking_show` (which would freeze the UI).
                            let menu_clone = toggle_tor_menu.clone();
                            let app_clone = app.clone();
                            app.dialog()
                                .message(msg)
                                .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancel)
                                .show(move |confirmed| {
                                    if !confirmed {
                                        return;
                                    }
                                    let _ = menu_clone.set_checked(enable);
                                    let state = app_clone.state::<tor::TorState>();
                                    if let Err(e) = tor::toggle_tor(&app_clone, &state, enable, None) {
                                        eprintln!("[qxchat] tray: failed to toggle Tor: {e}");
                                        let _ = menu_clone.set_checked(!enable);
                                    }
                                });
                        }

                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            ..
                        } = event
                        {
                            let app = tray.app_handle();

                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    })
                    .build(app)?;

                // Keep the tray "Connect through Tor" check state in sync when Tor
                // is started/stopped from the Settings UI (which emits tor:status).
                app.listen("tor:status", move |event| {
                    let running = serde_json::from_str::<serde_json::Value>(event.payload())
                        .ok()
                        .and_then(|v| v.get("running").and_then(|r| r.as_bool()))
                        .unwrap_or(false);
                    let _ = toggle_tor_sync.set_checked(running);
                });
            }

            // Linux WebKit permissions
            #[cfg(any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd"
            ))]
            {
                let window = app
                    .get_webview_window("main")
                    .expect("main window not found");

                window.with_webview(|webview| {
                    let webview = webview.inner();

                    if let Some(settings) = webview.settings() {
                        settings.set_enable_media(true);
                        settings.set_enable_media_stream(true);
                        settings.set_enable_webrtc(true);
                        settings.set_media_playback_requires_user_gesture(false);
                    }

                    webview.connect_permission_request(|_, request: &PermissionRequest| {
                        if request.is::<UserMediaPermissionRequest>()
                            || request.is::<NotificationPermissionRequest>()
                        {
                            request.allow();
                            return true;
                        }

                        false
                    });
                })?;
            }

            // macOS native title bar (traffic lights overlay) — instead of the
            // custom HTML title bar that Windows/Linux keep (decorations: false).
            #[cfg(target_os = "macos")]
            {
                let window = app
                    .get_webview_window("main")
                    .expect("main window not found");

                window.set_decorations(true)?;
                window.set_title_bar_style(tauri::TitleBarStyle::Overlay)?;
                window.set_shadow(true)?;
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Only intercept the main window's close (minimize-to-tray). The
            // splash must be allowed to close normally.
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();

                hide_window(window);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running QxChat");
}
