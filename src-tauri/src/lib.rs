#[cfg(not(any(target_os = "android", target_os = "ios")))]
use std::env;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

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
const GITHUB_RELEASES_API_URL: &str = "https://api.github.com/repos/lqxp/app/releases";
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const GITHUB_RELEASES_PAGE_URL: &str = "https://github.com/lqxp/app/releases";

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[derive(Debug, serde::Deserialize)]
struct GitHubRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GitHubReleaseAsset>,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[derive(Debug, serde::Deserialize)]
struct GitHubReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
struct AvailableUpdate {
    version: semver::Version,
    name: String,
    notes: Option<String>,
    page_url: String,
    download_url: String,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn parse_release_version(tag_name: &str) -> Option<semver::Version> {
    semver::Version::parse(tag_name.trim_start_matches(['v', 'V'])).ok()
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn asset_priority(name: &str) -> Option<u8> {
    let name = name.to_ascii_lowercase();

    #[cfg(target_os = "linux")]
    {
        if name.ends_with(".appimage") {
            return Some(0);
        }

        if name.ends_with(".deb") || name.ends_with(".rpm") {
            return Some(1);
        }

        if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            return Some(2);
        }
    }

    #[cfg(target_os = "windows")]
    {
        let is_current_arch = if cfg!(target_arch = "x86_64") {
            (name.contains("x64") || name.contains("x86_64") || name.contains("amd64"))
                && !name.contains("arm64")
                && !name.contains("aarch64")
        } else if cfg!(target_arch = "aarch64") {
            name.contains("arm64") || name.contains("aarch64")
        } else {
            false
        };

        if name.ends_with(".msi") && is_current_arch {
            return Some(0);
        }

        if name.ends_with(".msi") {
            return Some(1);
        }
        if name.ends_with(".exe") {
            return Some(2);
        }

        if name.ends_with(".zip") {
            return Some(3);
        }
    }

    #[cfg(target_os = "macos")]
    {
        if name.ends_with(".dmg") {
            return Some(0);
        }

        if name.ends_with(".app.tar.gz") || name.ends_with(".app.tar.gz.zip") {
            return Some(1);
        }
    }

    None
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn select_release_download_url(release: &GitHubRelease) -> String {
    release
        .assets
        .iter()
        .filter_map(|asset| asset_priority(&asset.name).map(|priority| (priority, asset)))
        .min_by_key(|(priority, _)| *priority)
        .map(|(_, asset)| asset.browser_download_url.clone())
        .unwrap_or_else(|| release.html_url.clone())
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn fetch_available_update() -> Result<Option<AvailableUpdate>, String> {
    let current_version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| format!("Invalid local version: {error}"))?;

    let releases = reqwest::Client::new()
        .get(GITHUB_RELEASES_API_URL)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")))
        .send()
        .await
        .map_err(|error| format!("Failed to fetch GitHub releases: {error}"))?
        .error_for_status()
        .map_err(|error| format!("GitHub rejected the update check: {error}"))?
        .json::<Vec<GitHubRelease>>()
        .await
        .map_err(|error| format!("Invalid GitHub response: {error}"))?;

    let update = releases
        .into_iter()
        .filter(|release| !release.draft && !release.prerelease)
        .filter_map(|release| parse_release_version(&release.tag_name).map(|version| (version, release)))
        .filter(|(version, _)| version > &current_version)
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(version, release)| AvailableUpdate {
            download_url: select_release_download_url(&release),
            name: release.name.clone().unwrap_or_else(|| release.tag_name.clone()),
            notes: release.body.clone().filter(|body| !body.trim().is_empty()),
            page_url: release.html_url.clone(),
            version,
        });

    Ok(update)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn show_update_check_error(app: tauri::AppHandle, error: String) {
    tauri_plugin_dialog::DialogExt::dialog(&app)
        .message(format!("The update check failed.\n\n{error}"))
        .title("Check Updates")
        .kind(tauri_plugin_dialog::MessageDialogKind::Error)
        .buttons(tauri_plugin_dialog::MessageDialogButtons::Ok)
        .show(|_| {});
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn show_no_update_available(app: tauri::AppHandle) {
    tauri_plugin_dialog::DialogExt::dialog(&app)
        .message(format!(
            "QxChat is already up to date.\n\nCurrent version: {}",
            env!("CARGO_PKG_VERSION")
        ))
        .title("Check Updates")
        .kind(tauri_plugin_dialog::MessageDialogKind::Info)
        .buttons(tauri_plugin_dialog::MessageDialogButtons::Ok)
        .show(|_| {});
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn prompt_update(app: tauri::AppHandle, update: AvailableUpdate) {
    let notes = update
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|notes| !notes.is_empty())
        .unwrap_or("No release notes.");

    let message = format!(
        "A new QxChat version is available.\n\nCurrent version: {}\nNew version: {} ({})\n\n{}\n\nDo you want to open the download now?",
        env!("CARGO_PKG_VERSION"),
        update.version,
        update.name,
        notes
    );

    let download_url = update.download_url;
    let page_url = update.page_url;
    tauri_plugin_dialog::DialogExt::dialog(&app)
        .message(message)
        .title("Update Available")
        .kind(tauri_plugin_dialog::MessageDialogKind::Info)
        .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancelCustom(
            "Update".into(),
            "Later".into(),
        ))
        .show(move |accepted| {
            if accepted {
                let url = if download_url == page_url {
                    GITHUB_RELEASES_PAGE_URL.to_string()
                } else {
                    download_url
                };

                if let Err(error) = tauri_plugin_opener::OpenerExt::opener(&app).open_url(url, None::<&str>) {
                    show_update_check_error(app, format!("Failed to open the download: {error}"));
                }
            }
        });
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn check_updates_from_tray(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit("qx:check-updates", ());
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init());

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
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
