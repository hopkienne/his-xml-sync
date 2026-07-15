use crate::{
    app_logger::{self, ExportLogsResult, LogInfo},
    db::AppDb,
    folder_watch,
    his_api::{self, HisAuthStatus},
    kr800_process::{self, Kr800ProcessState, PatientListSnapshot, ProcessResult},
    license::{self, LicenseInfo, LicenseStatus},
    settings::{self, AppSettings},
    sync::{self, SyncSummary},
    xml_parser::{self, XmlPreview},
    xml_track::{self, DeviceFolderState, PatientQueryParam, ScanResult, TrackedXmlFile},
};
use tauri::{AppHandle, State};

#[tauri::command]
pub fn get_license_status() -> LicenseStatus {
    app_logger::debug("license", "get_license_status");
    license::current_status()
}

#[tauri::command]
pub fn activate_license(key: String) -> Result<LicenseInfo, String> {
    app_logger::info(
        "license",
        &format!("activate_license key_len={}", key.trim().len()),
    );
    match license::activate(&key) {
        Ok(info) => {
            app_logger::info(
                "license",
                &format!(
                    "activate_license ok facility={} expires={}",
                    info.facility_name, info.expires_at
                ),
            );
            Ok(info)
        }
        Err(err) => {
            app_logger::error("license", &format!("activate_license failed: {err}"));
            Err(err)
        }
    }
}

#[tauri::command]
pub fn get_settings(db: State<'_, AppDb>) -> Result<AppSettings, String> {
    app_logger::debug("settings", "get_settings");
    match settings::load(&db) {
        Ok(s) => {
            app_logger::info(
                "settings",
                &format!(
                    "get_settings ok his_api_url={} ds_co_so_kcb_id={} username={}",
                    s.his_api_url, s.ds_co_so_kcb_id, s.username
                ),
            );
            Ok(s)
        }
        Err(err) => {
            app_logger::error("settings", &format!("get_settings failed: {err}"));
            Err(err)
        }
    }
}

#[tauri::command]
pub fn save_settings(db: State<'_, AppDb>, settings: AppSettings) -> Result<AppSettings, String> {
    app_logger::info(
        "settings",
        &format!(
            "save_settings request his_api_url={} ds_co_so_kcb_id={} username={} password_provided={}",
            settings.his_api_url,
            settings.ds_co_so_kcb_id,
            settings.username,
            !settings.password.is_empty()
        ),
    );
    match settings::save(&db, settings) {
        Ok(s) => {
            app_logger::info(
                "settings",
                &format!("save_settings ok updated_at={:?}", s.updated_at),
            );
            Ok(s)
        }
        Err(err) => {
            app_logger::error("settings", &format!("save_settings failed: {err}"));
            Err(err)
        }
    }
}

#[tauri::command]
pub fn preview_xml_file(path: String) -> Result<XmlPreview, String> {
    app_logger::info("xml_parser", &format!("preview_xml_file path={path}"));
    match xml_parser::preview_file(&path) {
        Ok(preview) => {
            app_logger::info(
                "xml_parser",
                &format!("preview_xml_file ok file={}", preview.file_name),
            );
            Ok(preview)
        }
        Err(err) => {
            app_logger::error("xml_parser", &format!("preview_xml_file failed: {err}"));
            Err(err)
        }
    }
}

#[tauri::command]
pub fn run_sync_once() -> Result<SyncSummary, String> {
    app_logger::info("sync", "run_sync_once start");
    match sync::run_once() {
        Ok(summary) => {
            app_logger::info(
                "sync",
                &format!(
                    "run_sync_once done scanned={} sent={} skipped={} failed={}",
                    summary.scanned_files,
                    summary.sent_results,
                    summary.skipped_files,
                    summary.failed_files
                ),
            );
            Ok(summary)
        }
        Err(err) => {
            app_logger::error("sync", &format!("run_sync_once failed: {err}"));
            Err(err)
        }
    }
}

#[tauri::command]
pub fn get_device_folder(
    db: State<'_, AppDb>,
    device_key: String,
) -> Result<DeviceFolderState, String> {
    app_logger::debug(
        "xml_track",
        &format!("get_device_folder device={device_key}"),
    );
    match xml_track::get_device_folder(&db, &device_key) {
        Ok(state) => {
            app_logger::info(
                "xml_track",
                &format!(
                    "get_device_folder ok device={} folder={:?} auto_process={}",
                    device_key, state.tracking_folder, state.auto_process_enabled
                ),
            );
            Ok(state)
        }
        Err(err) => {
            app_logger::error("xml_track", &format!("get_device_folder failed: {err}"));
            Err(err)
        }
    }
}

/// Query params API danh sách người bệnh (KR-800).
#[tauri::command]
pub fn get_patient_query_params(
    db: State<'_, AppDb>,
    device_key: String,
) -> Result<Vec<PatientQueryParam>, String> {
    app_logger::debug(
        "xml_track",
        &format!("get_patient_query_params device={device_key}"),
    );
    match xml_track::get_patient_query_params(&db, &device_key) {
        Ok(params) => {
            app_logger::info(
                "xml_track",
                &format!(
                    "get_patient_query_params ok device={} count={}",
                    device_key,
                    params.len()
                ),
            );
            Ok(params)
        }
        Err(err) => {
            app_logger::error(
                "xml_track",
                &format!("get_patient_query_params failed: {err}"),
            );
            Err(err)
        }
    }
}

/// Lưu query params API danh sách người bệnh (KR-800).
#[tauri::command]
pub fn save_patient_query_params(
    db: State<'_, AppDb>,
    device_key: String,
    params: Vec<PatientQueryParam>,
) -> Result<Vec<PatientQueryParam>, String> {
    app_logger::info(
        "xml_track",
        &format!(
            "save_patient_query_params device={} count={}",
            device_key,
            params.len()
        ),
    );
    match xml_track::save_patient_query_params(&db, &device_key, params) {
        Ok(saved) => {
            app_logger::info(
                "xml_track",
                &format!(
                    "save_patient_query_params ok device={} count={}",
                    device_key,
                    saved.len()
                ),
            );
            Ok(saved)
        }
        Err(err) => {
            app_logger::error(
                "xml_track",
                &format!("save_patient_query_params failed: {err}"),
            );
            Err(err)
        }
    }
}

/// Bật/tắt tự động xử lý HIS cho KR-800. Khi bật và đã có folder → kick process waiting ngay.
#[tauri::command]
pub async fn set_auto_process_enabled(
    app: AppHandle,
    db: State<'_, AppDb>,
    device_key: String,
    enabled: bool,
) -> Result<DeviceFolderState, String> {
    app_logger::info(
        "xml_track",
        &format!("set_auto_process_enabled device={device_key} enabled={enabled}"),
    );
    let state = match xml_track::set_auto_process_enabled(&db, &device_key, enabled) {
        Ok(s) => s,
        Err(err) => {
            app_logger::error(
                "xml_track",
                &format!("set_auto_process_enabled failed: {err}"),
            );
            return Err(err);
        }
    };

    if enabled {
        if state
            .tracking_folder
            .as_ref()
            .map(|f| !f.trim().is_empty())
            .unwrap_or(false)
        {
            folder_watch::trigger_auto_process_now(&app).await;
        } else {
            app_logger::info(
                "xml_track",
                "auto_process bật nhưng chưa có tracking folder — chờ user chọn folder",
            );
        }
    }

    Ok(state)
}

#[tauri::command]
pub fn set_tracking_folder_and_scan(
    app: AppHandle,
    db: State<'_, AppDb>,
    device_key: String,
    folder: String,
) -> Result<ScanResult, String> {
    app_logger::info(
        "xml_track",
        &format!("set_tracking_folder_and_scan device={device_key} folder={folder}"),
    );
    match xml_track::set_tracking_folder_and_scan(Some(&app), &db, &device_key, &folder) {
        Ok(result) => {
            // Nếu user đã bật tự xử lý: kick pipeline sau khi có folder (không chờ poll).
            if xml_track::is_auto_process_enabled(&db, &device_key) {
                let app_clone = app.clone();
                tauri::async_runtime::spawn(async move {
                    folder_watch::trigger_auto_process_now(&app_clone).await;
                });
            }
            app_logger::info(
                "xml_track",
                &format!(
                    "set_tracking_folder_and_scan ok scanned={} inserted={} updated={} pruned={} prune_skipped={} tracked={}",
                    result.scanned_count,
                    result.inserted_count,
                    result.updated_count,
                    result.pruned_count,
                    result.prune_skipped,
                    result.tracked_count
                ),
            );
            Ok(result)
        }
        Err(err) => {
            app_logger::error(
                "xml_track",
                &format!("set_tracking_folder_and_scan failed: {err}"),
            );
            Err(err)
        }
    }
}

#[tauri::command]
pub fn rescan_tracking_folder(
    app: AppHandle,
    db: State<'_, AppDb>,
    device_key: String,
) -> Result<ScanResult, String> {
    app_logger::info(
        "xml_track",
        &format!("rescan_tracking_folder device={device_key}"),
    );
    match xml_track::rescan_tracking_folder(Some(&app), &db, &device_key) {
        Ok(result) => {
            app_logger::info(
                "xml_track",
                &format!(
                    "rescan_tracking_folder ok scanned={} inserted={} updated={} pruned={} prune_skipped={} tracked={}",
                    result.scanned_count,
                    result.inserted_count,
                    result.updated_count,
                    result.pruned_count,
                    result.prune_skipped,
                    result.tracked_count
                ),
            );
            Ok(result)
        }
        Err(err) => {
            app_logger::error(
                "xml_track",
                &format!("rescan_tracking_folder failed: {err}"),
            );
            Err(err)
        }
    }
}

/// `from_time` / `to_time`: `YYYY-MM-DD HH:mm:ss` — lọc theo `created_at`.
/// Bắt buộc truyền cả hai từ UI; không truyền → trả mảng rỗng (không load full table).
#[tauri::command]
pub fn list_xml_files(
    db: State<'_, AppDb>,
    device_key: String,
    from_time: Option<String>,
    to_time: Option<String>,
) -> Result<Vec<TrackedXmlFile>, String> {
    app_logger::debug(
        "xml_track",
        &format!(
            "list_xml_files device={device_key} from={:?} to={:?}",
            from_time, to_time
        ),
    );
    match xml_track::list_xml_files(
        &db,
        &device_key,
        from_time.as_deref(),
        to_time.as_deref(),
    ) {
        Ok(files) => {
            app_logger::info(
                "xml_track",
                &format!(
                    "list_xml_files ok device={device_key} count={}",
                    files.len()
                ),
            );
            Ok(files)
        }
        Err(err) => {
            app_logger::error("xml_track", &format!("list_xml_files failed: {err}"));
            Err(err)
        }
    }
}

/// JSON danh sách người bệnh từ lần gọi API thành công gần nhất (phiên app).
#[tauri::command]
pub async fn get_last_patient_list(
    process_state: State<'_, Kr800ProcessState>,
) -> Result<Option<PatientListSnapshot>, String> {
    Ok(kr800_process::get_last_patient_list(&process_state).await)
}

#[tauri::command]
pub async fn process_kr800(
    app: AppHandle,
    db: State<'_, AppDb>,
    process_state: State<'_, Kr800ProcessState>,
    device_key: String,
    from_time: String,
    to_time: String,
) -> Result<ProcessResult, String> {
    if device_key != "kr-800" {
        return Err(format!("Thiết bị chưa được hỗ trợ: {device_key}"));
    }
    app_logger::info(
        "kr800",
        &format!("process start from={from_time} to={to_time}"),
    );
    let result = kr800_process::process(&app, &db, &process_state, &from_time, &to_time).await;
    match &result {
        Ok(summary) => app_logger::info(
            "kr800",
            &format!(
                "process done total={} processed={} failed={} skipped={}",
                summary.total, summary.processed, summary.failed, summary.skipped
            ),
        ),
        Err(error) => app_logger::error("kr800", &format!("process failed: {error}")),
    }
    result
}

#[tauri::command]
pub fn get_log_info() -> Result<LogInfo, String> {
    app_logger::get_info()
}

#[tauri::command]
pub fn export_app_logs(target_path: String) -> Result<ExportLogsResult, String> {
    app_logger::info("app", &format!("export_app_logs target={target_path}"));
    match app_logger::export_to(&target_path) {
        Ok(result) => Ok(result),
        Err(err) => {
            app_logger::error("app", &format!("export_app_logs failed: {err}"));
            Err(err)
        }
    }
}

#[tauri::command]
pub fn log_client_event(level: String, module: String, message: String) {
    app_logger::log_from_frontend(&level, &module, &message);
}

/// Đăng nhập HIS bằng tài khoản trong app_config, lưu access_token vào auth_session.
#[tauri::command]
pub async fn login_his(db: State<'_, AppDb>) -> Result<HisAuthStatus, String> {
    app_logger::info("his_api", "login_his command start");
    match his_api::login_and_store(&db).await {
        Ok(status) => {
            app_logger::info(
                "his_api",
                &format!(
                    "login_his ok logged_in={} username={:?}",
                    status.logged_in, status.username
                ),
            );
            Ok(status)
        }
        Err(err) => {
            app_logger::error("his_api", &format!("login_his failed: {err}"));
            Err(err)
        }
    }
}

#[tauri::command]
pub fn get_auth_status(db: State<'_, AppDb>) -> Result<HisAuthStatus, String> {
    app_logger::debug("his_api", "get_auth_status");
    his_api::get_auth_status(&db)
}
