mod commands;
mod his_api;
mod license;
pub mod license_core;
mod settings;
mod sync;
mod tray;
mod xml_parser;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            tray::setup(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Err(error) = window.hide() {
                    eprintln!("failed to hide window: {error}");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_license_status,
            commands::activate_license,
            commands::get_settings,
            commands::save_settings,
            commands::preview_xml_file,
            commands::run_sync_once
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
