mod app_logger;
mod commands;
mod ct800;
mod db;
mod folder_watch;
mod his_api;
mod hdr9000;
mod kr800_process;
mod license;
pub mod license_core;
mod measurement_pair;
mod refraction_catalog;
mod settings;
mod sync;
mod tray;
mod xml_parser;
mod xml_track;

use tauri::Manager;
use tauri_plugin_autostart::MacosLauncher;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        // Đăng ký app vào startup OS (Windows registry / macOS LaunchAgent / Linux).
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            tray::setup(app)?;

            let app_data = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("app_data_dir: {e}"))?;
            std::fs::create_dir_all(&app_data)
                .map_err(|e| format!("create app_data_dir: {e}"))?;

            app_logger::init(&app_data)?;

            let database = db::init(app.handle())?;
            app_logger::info(
                "db",
                &format!("SQLite ready: {}", database.path.display()),
            );
            app.manage(database);
            app.manage(kr800_process::Kr800ProcessState::default());
            app.manage(hdr9000::Hdr9000ProcessState::default());
            app.manage(ct800::Ct800ProcessState::default());
            // Tự quét nền folder tracking + tự xử lý file waiting.
            folder_watch::start(app.handle().clone());
            hdr9000::start_watch(app.handle().clone());
            ct800::start_watch(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                app_logger::info("app", "Window close requested → hide to tray");
                api.prevent_close();
                if let Err(error) = window.hide() {
                    app_logger::error("app", &format!("failed to hide window: {error}"));
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
            commands::run_sync_once,
            commands::get_device_folder,
            commands::get_patient_query_params,
            commands::save_patient_query_params,
            commands::set_auto_process_enabled,
            commands::set_tracking_folder_and_scan,
            commands::rescan_tracking_folder,
            commands::list_xml_files,
            commands::get_log_info,
            commands::export_app_logs,
            commands::log_client_event,
            commands::login_his,
            commands::get_auth_status,
            commands::get_last_patient_list,
            commands::process_kr800,
            commands::process_hdr9000,
            commands::process_ct800,
            commands::get_ct800_revision_detail
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
