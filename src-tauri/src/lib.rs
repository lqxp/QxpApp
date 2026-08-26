#[cfg(not(any(target_os = "android", target_os = "ios")))]
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Emitter, Listener, Manager, WebviewUrl, WebviewWindowBuilder,
};

pub mod permissions;
pub mod background;
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
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                // ── Startup: build the WebView *after* Tor is ready ──────────
                // On Windows the WebView2 proxy (--proxy-server) must be set at
                // environment creation time, so we start Tor (if enabled) before
                // creating the main window, showing a lightweight splash meanwhile.
                let tor_enabled = tor::read_tor_enabled(app.handle());

                let splash = WebviewWindowBuilder::new(app, "splash", WebviewUrl::App("splash.html".into()))
                    .title("QxChat")
                    .inner_size(360.0, 260.0)
                    .resizable(false)
                    .decorations(false)
                    .always_on_top(true)
                    .build()?;

                // Determine the SOCKS port (default 9050).
                let port = app
                    .try_state::<tor::TorState>()
                    .map(|s| s.port())
                    .unwrap_or(9050);

                if tor_enabled {
                    let result = {
                        let state = app.try_state::<tor::TorState>();
                        match state {
                            Some(s) => tor::start_tor_blocking(
                                app.handle(),
                                s.inner(),
                                Some(port),
                                std::time::Duration::from_secs(60),
                            ),
                            None => Err("TorState not ready".into()),
                        }
                    };
                    if let Err(e) = result {
                        eprintln!("[qxchat] boot: failed to start Tor: {e}");
                    }
                }

                let mut main_builder = WebviewWindowBuilder::new(
                    app,
                    "main",
                    WebviewUrl::App("index.html".into()),
                )
                .title("QxChat")
                .inner_size(1200.0, 800.0)
                .decorations(false);

                #[cfg(target_os = "windows")]
                if tor_enabled {
                    main_builder = main_builder.additional_browser_args(&format!(
                        "--proxy-server=socks5://127.0.0.1:{port}"
                    ));
                }

                main_builder.build()?;
                let _ = splash.close();

                // Linux/macOS proxy is set at runtime after the window exists.
                #[cfg(any(
                    target_os = "linux",
                    target_os = "dragonfly",
                    target_os = "freebsd",
                    target_os = "netbsd",
                    target_os = "openbsd"
                ))]
                if tor_enabled {
                    let _ = tor::apply_proxy(app.handle(), port);
                }

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
                    tor_enabled,
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
                            let state = app.state::<tor::TorState>();
                            if state.running() {
                                let _ = tor::stop_tor(&app, &state);
                                let _ = toggle_tor_menu.set_checked(false);
                            } else {
                                match tor::start_tor(&app, &state, None) {
                                    Ok(_) => {
                                        let _ = toggle_tor_menu.set_checked(true);
                                    }
                                    Err(e) => eprintln!("[qxchat] tray: failed to start Tor: {e}"),
                                }
                            }
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
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();

                hide_window(window);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running QxChat");
}
