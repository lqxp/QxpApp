fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .plugin(
                "tor",
                tauri_build::InlinedPlugin::new()
                    .commands(&["status", "start", "stop", "is_ready", "relays", "circuit", "geo", "geo_ip"]),
            )
            .plugin(
                "screen-audio",
                tauri_build::InlinedPlugin::new().commands(&["start", "stop"]),
            )
            .plugin(
                "background",
                tauri_build::InlinedPlugin::new()
                    .commands(&["start_background", "stop_background", "is_background_running"]),
            ),
    )
    .expect("failed to run tauri-build");
}
