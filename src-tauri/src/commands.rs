use crate::{
    license::{self, LicenseInfo, LicenseStatus},
    settings::{self, AppSettings},
    sync::{self, SyncSummary},
    xml_parser::{self, XmlPreview},
};

#[tauri::command]
pub fn get_license_status() -> LicenseStatus {
    license::current_status()
}

#[tauri::command]
pub fn activate_license(key: String) -> Result<LicenseInfo, String> {
    license::activate(&key)
}

#[tauri::command]
pub fn get_settings() -> AppSettings {
    settings::load()
}

#[tauri::command]
pub fn save_settings(settings: AppSettings) -> Result<AppSettings, String> {
    settings::save(settings)
}

#[tauri::command]
pub fn preview_xml_file(path: String) -> Result<XmlPreview, String> {
    xml_parser::preview_file(&path)
}

#[tauri::command]
pub fn run_sync_once() -> Result<SyncSummary, String> {
    sync::run_once()
}
