#[cfg(not(any(target_os = "android", target_os = "ios")))]
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
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
                let quit = MenuItem::with_id(app, "quit", "Quit QxChat", true, None::<&str>)?;

                let show = MenuItem::with_id(app, "show", "Open QxChat", true, None::<&str>)?;
                let check_updates = MenuItem::with_id(
                    app,
                    "check_updates",
                    "Check Updates",
                    true,
                    None::<&str>,
                )?;

                let menu = Menu::with_items(app, &[&show, &check_updates, &quit])?;

                TrayIconBuilder::new()
                    .icon(app.default_window_icon().expect("missing app icon").clone())
                    .menu(&menu)
                    .on_menu_event(|app, event| match event.id.as_ref() {
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
