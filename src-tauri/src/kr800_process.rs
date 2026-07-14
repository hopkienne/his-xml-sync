use crate::app_logger;
use crate::db::AppDb;
use crate::his_api;
use crate::settings::{self, AppSettings};
use crate::xml_parser::{self, ParsedEye};
use crate::xml_track::{self, TrackedXmlFile};
use futures::stream::{self, StreamExt};
use reqwest::{Client, StatusCode};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use tokio::time::sleep;

const DEVICE_KEY: &str = "kr-800";
const PATIENT_PATH: &str = "/api/his/v1/nb-kham-ck-mat/nguoi-benh";
const UPDATE_PATH: &str = "/api/his/v1/nb-kham-ck-mat";
const MAX_CONCURRENT_FILES: usize = 5;
const MAX_TRANSIENT_RETRIES: u32 = 3;
const FILE_PROGRESS_EVENT: &str = "kr800:file-progress";

#[derive(Default)]
pub struct Kr800ProcessState {
    run_lock: Mutex<()>,
    token_lock: Mutex<()>,
    patient_cache: Mutex<Option<PatientCache>>,
    hash_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

#[derive(Clone)]
struct PatientCache {
    key: PatientCacheKey,
    index: Arc<PatientIndex>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatientCacheKey {
    from_time: String,
    to_time: String,
    facility_id: i64,
    api_url: String,
    username: String,
}

type PatientIndex = HashMap<String, Vec<Option<i64>>>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessResult {
    pub total: usize,
    pub processed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub files: Vec<TrackedXmlFile>,
}

#[derive(Debug, Deserialize)]
struct PatientEnvelope {
    data: Vec<PatientRow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatientRow {
    ma_ho_so: String,
    nb_dot_dieu_tri_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CatalogEntry {
    id: i64,
    name: String,
    kind: String,
}

struct Catalog {
    sph: HashMap<i32, i64>,
    cyl: HashMap<i32, i64>,
    axis: HashMap<i64, i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct EyePayload {
    sph_id: i64,
    cyl_id: i64,
    ax_id: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HisPayload {
    mat_phai_khuc_xa: EyePayload,
    mat_trai_khuc_xa: EyePayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    mat_phai_kinh_moi: Option<EyePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mat_trai_kinh_moi: Option<EyePayload>,
}

struct WorkFile {
    id: i64,
    path: String,
}

#[derive(Default)]
struct FileOutcome {
    processed: bool,
    skipped: bool,
}

pub async fn process(
    app: &AppHandle,
    db: &AppDb,
    state: &Kr800ProcessState,
    from_time: &str,
    to_time: &str,
) -> Result<ProcessResult, String> {
    process_inner(app, db, state, from_time, to_time, true).await
}

/// Auto-process nền: bỏ qua nếu pipeline đang chạy (không xếp hàng chờ).
/// Trả `Ok(None)` khi bận.
pub async fn try_process(
    app: &AppHandle,
    db: &AppDb,
    state: &Kr800ProcessState,
    from_time: &str,
    to_time: &str,
) -> Result<Option<ProcessResult>, String> {
    process_inner(app, db, state, from_time, to_time, false)
        .await
        .map(Some)
        .or_else(|err| {
            if err == BUSY_MSG {
                Ok(None)
            } else {
                Err(err)
            }
        })
}

const BUSY_MSG: &str = "__process_busy__";

async fn process_inner(
    app: &AppHandle,
    db: &AppDb,
    state: &Kr800ProcessState,
    from_time: &str,
    to_time: &str,
    wait_if_busy: bool,
) -> Result<ProcessResult, String> {
    validate_range(from_time, to_time)?;
    let _run_guard = if wait_if_busy {
        state.run_lock.lock().await
    } else {
        match state.run_lock.try_lock() {
            Ok(guard) => guard,
            Err(_) => return Err(BUSY_MSG.into()),
        }
    };
    state.hash_locks.lock().await.clear();
    let settings = settings::load(db)?;
    if settings.his_api_url.trim().is_empty() {
        return Err("Chưa cấu hình API URL HIS. Vào Cấu hình để lưu trước.".into());
    }
    if settings.username.trim().is_empty() {
        return Err("Chưa cấu hình tài khoản HIS.".into());
    }
    let catalog = catalog()?;
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("Không tạo được HTTP client: {error}"))?;
    let patients = patient_index(db, state, &client, &settings, from_time, to_time).await?;
    let work = waiting_files(db, from_time, to_time)?;
    let total = work.len();

    let outcomes = stream::iter(
        work.into_iter()
            .map(|file| process_one(app, db, state, &client, &settings, &patients, catalog, file)),
    )
    .buffer_unordered(MAX_CONCURRENT_FILES)
    .collect::<Vec<_>>()
    .await;

    let processed = outcomes.iter().filter(|outcome| outcome.processed).count();
    let skipped = outcomes.iter().filter(|outcome| outcome.skipped).count();
    let failed = total.saturating_sub(processed + skipped);
    // Chỉ trả file trong khoảng xử lý — không load full table về UI.
    let files = xml_track::list_xml_files(db, DEVICE_KEY, Some(from_time), Some(to_time))?;
    Ok(ProcessResult {
        total,
        processed,
        failed,
        skipped,
        files,
    })
}

async fn process_one(
    app: &AppHandle,
    db: &AppDb,
    state: &Kr800ProcessState,
    client: &Client,
    settings: &AppSettings,
    patients: &PatientIndex,
    catalog: &Catalog,
    file: WorkFile,
) -> FileOutcome {
    if !claim_file(db, file.id).unwrap_or(false) {
        return FileOutcome::default();
    }
    emit_file_progress(app, db, file.id);
    match process_claimed(app, db, state, client, settings, patients, catalog, &file).await {
        Ok(outcome) => outcome,
        Err((status, message)) => {
            let _ = fail_file(db, file.id, status, &message);
            emit_file_progress(app, db, file.id);
            app_logger::error(
                "kr800",
                &format!("file_id={} status={} error={}", file.id, status, message),
            );
            FileOutcome::default()
        }
    }
}

async fn process_claimed(
    app: &AppHandle,
    db: &AppDb,
    state: &Kr800ProcessState,
    client: &Client,
    settings: &AppSettings,
    patients: &PatientIndex,
    catalog: &Catalog,
    file: &WorkFile,
) -> Result<FileOutcome, (&'static str, String)> {
    let bytes = tokio::fs::read(&file.path)
        .await
        .map_err(|error| ("xml_error", format!("Không đọc được XML: {error}")))?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    save_hash(db, file.id, &hash).map_err(|error| ("xml_error", error))?;
    // Hai file khác đường dẫn nhưng cùng nội dung có thể được claim đồng thời.
    // Giữ khóa theo hash đến khi file đầu hoàn tất để file sau nhìn thấy trạng thái processed.
    let hash_lock = {
        let mut locks = state.hash_locks.lock().await;
        Arc::clone(
            locks
                .entry(hash.clone())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    };
    let _hash_guard = hash_lock.lock().await;
    if duplicate_processed(db, file.id, &hash).map_err(|error| ("xml_error", error))? {
        mark_duplicate(db, file.id, &hash).map_err(|error| ("xml_error", error))?;
        emit_file_progress(app, db, file.id);
        return Ok(FileOutcome {
            skipped: true,
            ..FileOutcome::default()
        });
    }

    let parsed = xml_parser::parse_measurement(&bytes).map_err(|error| ("xml_error", error))?;
    set_stage(db, file.id, "parsed").map_err(|error| ("xml_error", error))?;
    emit_file_progress(app, db, file.id);
    let treatment_id = match_treatment(patients, &parsed.patient_id)?;
    save_patient(db, file.id, &parsed.patient_id, treatment_id)
        .map_err(|error| ("treatment_ambiguous", error))?;
    set_stage(db, file.id, "patient_matched").map_err(|error| ("treatment_ambiguous", error))?;
    emit_file_progress(app, db, file.id);

    let right = map_eye(catalog, &parsed.right).map_err(|error| ("mapping_error", error))?;
    let left = map_eye(catalog, &parsed.left).map_err(|error| ("mapping_error", error))?;
    let payload = HisPayload {
        mat_phai_khuc_xa: right.clone(),
        mat_trai_khuc_xa: left.clone(),
        mat_phai_kinh_moi: settings.copy_refraction_to_new_glasses.then_some(right),
        mat_trai_kinh_moi: settings.copy_refraction_to_new_glasses.then_some(left),
    };
    let payload_json = serde_json::to_string(&payload).map_err(|error| {
        (
            "mapping_error",
            format!("Serialize request thất bại: {error}"),
        )
    })?;
    save_request(db, file.id, &payload_json).map_err(|error| ("mapping_error", error))?;
    set_stage(db, file.id, "mapped").map_err(|error| ("mapping_error", error))?;
    emit_file_progress(app, db, file.id);
    set_stage(db, file.id, "sending").map_err(|error| ("send_error", error))?;
    emit_file_progress(app, db, file.id);
    let response = send_update(db, state, client, settings, treatment_id, &payload, file.id)
        .await
        .map_err(|error| ("send_error", error))?;
    finish_file(db, file.id, &response).map_err(|error| ("send_error", error))?;
    emit_file_progress(app, db, file.id);
    app_logger::info(
        "kr800",
        &format!(
            "file_id={} patient={} treatment_id={} processed",
            file.id, parsed.patient_id, treatment_id
        ),
    );
    Ok(FileOutcome {
        processed: true,
        ..FileOutcome::default()
    })
}

fn emit_file_progress(app: &AppHandle, db: &AppDb, file_id: i64) {
    let result = xml_track::get_xml_file(db, file_id)
        .and_then(|file| file.ok_or_else(|| format!("Không tìm thấy XML id={file_id}.")))
        .and_then(|file| {
            app.emit(FILE_PROGRESS_EVENT, file)
                .map_err(|error| format!("Phát event tiến độ XML thất bại: {error}"))
        });
    if let Err(error) = result {
        app_logger::warn("kr800", &error);
    }
}

fn match_treatment(
    patients: &PatientIndex,
    patient_id: &str,
) -> Result<i64, (&'static str, String)> {
    // So sánh không phân biệt hoa/thường (XML ID vs maHoSo từ API).
    let key = normalize_patient_code(patient_id);
    match patients.get(&key) {
        None => Err((
            "patient_not_found",
            format!("Không tìm thấy bệnh nhân có mã hồ sơ {patient_id}."),
        )),
        Some(values) if values.len() > 1 => Err((
            "treatment_ambiguous",
            format!(
                "Tìm thấy {} đợt điều trị cho mã hồ sơ {}.",
                values.len(),
                patient_id
            ),
        )),
        Some(values) if values.is_empty() => Err((
            "treatment_ambiguous",
            format!("Bệnh nhân {patient_id} không có đợt điều trị."),
        )),
        Some(values) => values[0].ok_or_else(|| {
            (
                "treatment_ambiguous",
                format!("Bệnh nhân {patient_id} không có nbDotDieuTriId."),
            )
        }),
    }
}

/// Chuẩn hoá mã hồ sơ để so khớp: trim + lowercase.
fn normalize_patient_code(value: &str) -> String {
    value.trim().to_lowercase()
}

async fn patient_index(
    db: &AppDb,
    state: &Kr800ProcessState,
    client: &Client,
    settings: &AppSettings,
    from_time: &str,
    to_time: &str,
) -> Result<Arc<PatientIndex>, String> {
    let key = PatientCacheKey {
        from_time: from_time.to_string(),
        to_time: to_time.to_string(),
        facility_id: settings.ds_co_so_kcb_id,
        api_url: settings.his_api_url.trim().to_string(),
        username: settings.username.trim().to_string(),
    };
    if let Some(cache) = state.patient_cache.lock().await.as_ref() {
        if cache.key == key {
            return Ok(Arc::clone(&cache.index));
        }
    }
    let url = his_api::join_url(&settings.his_api_url, PATIENT_PATH);
    let mut token = ensure_token(db, state).await?;
    let mut response = fetch_patients(client, &url, &token, &key).await?;
    if response.status() == StatusCode::UNAUTHORIZED {
        token = refresh_token(db, state, &token).await?;
        response = fetch_patients(client, &url, &token, &key).await?;
    }
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Không đọc được response người bệnh: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "API danh sách người bệnh trả về {}: {}",
            status,
            preview(&body)
        ));
    }
    let envelope: PatientEnvelope = serde_json::from_str(&body)
        .map_err(|error| format!("Response người bệnh không hợp lệ: {error}"))?;
    let mut index: PatientIndex = HashMap::new();
    for patient in envelope.data {
        // Key lowercase để khớp nsCommon:ID / maHoSo không phân biệt hoa-thường.
        index
            .entry(normalize_patient_code(&patient.ma_ho_so))
            .or_default()
            .push(patient.nb_dot_dieu_tri_id);
    }
    let index = Arc::new(index);
    *state.patient_cache.lock().await = Some(PatientCache {
        key,
        index: Arc::clone(&index),
    });
    Ok(index)
}

async fn fetch_patients(
    client: &Client,
    url: &str,
    token: &str,
    key: &PatientCacheKey,
) -> Result<reqwest::Response, String> {
    client
        .get(url)
        .bearer_auth(token)
        .query(&[
            ("page", "0".to_string()),
            ("sort", "thoiGianVaoVien,asc".to_string()),
            ("size", "9999".to_string()),
            ("tuThoiGianVaoVien", key.from_time.clone()),
            ("denThoiGianVaoVien", key.to_time.clone()),
            ("dsTrangThai", "10".to_string()),
            ("theoPhongKham", "false".to_string()),
            ("dsCoSoKcbId", key.facility_id.to_string()),
        ])
        .send()
        .await
        .map_err(|error| format!("Gọi API danh sách người bệnh thất bại: {error}"))
}

async fn send_update(
    db: &AppDb,
    state: &Kr800ProcessState,
    client: &Client,
    settings: &AppSettings,
    treatment_id: i64,
    payload: &HisPayload,
    file_id: i64,
) -> Result<String, String> {
    let url = format!(
        "{}/{}",
        his_api::join_url(&settings.his_api_url, UPDATE_PATH),
        treatment_id
    );
    let mut token = ensure_token(db, state).await?;
    let mut auth_retried = false;
    let mut transient_retries = 0u32;
    loop {
        increment_attempt(db, file_id)?;
        let response = client
            .post(&url)
            .bearer_auth(&token)
            .json(payload)
            .send()
            .await;
        match response {
            Ok(response) => {
                let status = response.status();
                let body = response
                    .text()
                    .await
                    .map_err(|error| format!("Không đọc được response HIS: {error}"))?;
                if status.is_success() {
                    return Ok(body);
                }
                if status == StatusCode::UNAUTHORIZED && !auth_retried {
                    auth_retried = true;
                    token = refresh_token(db, state, &token).await?;
                    continue;
                }
                if is_transient(status) && transient_retries < MAX_TRANSIENT_RETRIES {
                    sleep(Duration::from_secs(1 << transient_retries)).await;
                    transient_retries += 1;
                    continue;
                }
                return Err(format!("HIS trả về {}: {}", status, preview(&body)));
            }
            Err(error) if error.is_builder() => {
                return Err(format!("Tạo request HIS thất bại: {error}"));
            }
            Err(_) if transient_retries < MAX_TRANSIENT_RETRIES => {
                sleep(Duration::from_secs(1 << transient_retries)).await;
                transient_retries += 1;
            }
            Err(error) => return Err(format!("Gửi HIS thất bại: {error}")),
        }
    }
}

async fn ensure_token(db: &AppDb, state: &Kr800ProcessState) -> Result<String, String> {
    if let Some(token) = his_api::get_access_token(db)? {
        return Ok(token);
    }
    let _guard = state.token_lock.lock().await;
    if let Some(token) = his_api::get_access_token(db)? {
        return Ok(token);
    }
    his_api::login_and_store(db).await?;
    his_api::get_access_token(db)?.ok_or_else(|| "Login xong nhưng không có access_token.".into())
}

async fn refresh_token(
    db: &AppDb,
    state: &Kr800ProcessState,
    stale_token: &str,
) -> Result<String, String> {
    let _guard = state.token_lock.lock().await;
    if let Some(current) = his_api::get_access_token(db)? {
        if current != stale_token {
            return Ok(current);
        }
    }
    his_api::login_and_store(db).await?;
    his_api::get_access_token(db)?.ok_or_else(|| "Login lại nhưng không có access_token.".into())
}

fn catalog() -> Result<&'static Catalog, String> {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    if let Some(catalog) = CATALOG.get() {
        return Ok(catalog);
    }
    let entries: Vec<CatalogEntry> =
        serde_json::from_str(include_str!("../resources/dm_thi_luc.json"))
            .map_err(|error| format!("Danh mục thị lực không hợp lệ: {error}"))?;
    let mut parsed = Catalog {
        sph: HashMap::new(),
        cyl: HashMap::new(),
        axis: HashMap::new(),
    };
    for entry in entries {
        match entry.kind.as_str() {
            "SPH" => {
                if let Ok(value) = decimal_key(&entry.name) {
                    insert_unique(&mut parsed.sph, value, entry.id, "SPH")?;
                }
            }
            "CYL" => {
                if let Ok(value) = decimal_key(&entry.name) {
                    insert_unique(&mut parsed.cyl, value, entry.id, "CYL")?;
                }
            }
            "Axis" => {
                if let Ok(axis) = entry.name.parse::<i64>() {
                    insert_unique(&mut parsed.axis, axis, entry.id, "Axis")?;
                }
            }
            _ => {}
        }
    }
    let _ = CATALOG.set(parsed);
    CATALOG
        .get()
        .ok_or_else(|| "Không khởi tạo được danh mục thị lực.".into())
}

fn decimal_key(value: &str) -> Result<i32, String> {
    let parsed = value
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("Giá trị danh mục không phải số: {value}"))?;
    Ok((parsed * 100.0).round() as i32)
}

fn insert_unique<K: std::hash::Hash + Eq + Copy + std::fmt::Display>(
    map: &mut HashMap<K, i64>,
    key: K,
    id: i64,
    kind: &str,
) -> Result<(), String> {
    if let Some(existing) = map.insert(key, id) {
        return Err(format!(
            "Danh mục {kind} trùng giá trị {key}: ID {existing} và {id}"
        ));
    }
    Ok(())
}

fn map_eye(catalog: &Catalog, eye: &ParsedEye) -> Result<EyePayload, String> {
    let sph_key = (eye.sphere * 100.0).round() as i32;
    let cyl_key = (eye.cylinder * 100.0).round() as i32;
    Ok(EyePayload {
        sph_id: *catalog
            .sph
            .get(&sph_key)
            .ok_or_else(|| format!("Không tìm thấy danh mục SPH cho giá trị {:.2}", eye.sphere))?,
        cyl_id: *catalog.cyl.get(&cyl_key).ok_or_else(|| {
            format!(
                "Không tìm thấy danh mục CYL cho giá trị {:.2}",
                eye.cylinder
            )
        })?,
        ax_id: *catalog
            .axis
            .get(&eye.axis)
            .ok_or_else(|| format!("Không tìm thấy danh mục Axis cho giá trị {}", eye.axis))?,
    })
}

fn validate_range(from_time: &str, to_time: &str) -> Result<(), String> {
    let from = chrono::NaiveDateTime::parse_from_str(from_time, "%Y-%m-%d %H:%M:%S")
        .map_err(|_| "Thời gian bắt đầu không hợp lệ.".to_string())?;
    let to = chrono::NaiveDateTime::parse_from_str(to_time, "%Y-%m-%d %H:%M:%S")
        .map_err(|_| "Thời gian kết thúc không hợp lệ.".to_string())?;
    if from > to {
        return Err("Thời gian bắt đầu phải nhỏ hơn hoặc bằng thời gian kết thúc.".into());
    }
    Ok(())
}

fn waiting_files(db: &AppDb, from_time: &str, to_time: &str) -> Result<Vec<WorkFile>, String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    let mut statement = conn
        .prepare("SELECT id, file_path FROM xml_files WHERE device_key = ?1 AND status = 'waiting' AND created_at BETWEEN ?2 AND ?3 ORDER BY created_at, id")
        .map_err(|error| format!("Prepare queue XML thất bại: {error}"))?;
    let rows = statement
        .query_map(params![DEVICE_KEY, from_time, to_time], |row| {
            Ok(WorkFile {
                id: row.get(0)?,
                path: row.get(1)?,
            })
        })
        .map_err(|error| format!("Đọc queue XML thất bại: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Map queue XML thất bại: {error}"))
}

fn claim_file(db: &AppDb, id: i64) -> Result<bool, String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    let changed = conn
        .execute("UPDATE xml_files SET status = 'processing', error_message = NULL, attempt_count = 0, updated_at = datetime('now') WHERE id = ?1 AND status = 'waiting'", params![id])
        .map_err(|error| format!("Claim XML id={id} thất bại: {error}"))?;
    Ok(changed == 1)
}

fn set_stage(db: &AppDb, id: i64, status: &str) -> Result<(), String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    conn.execute(
        "UPDATE xml_files SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![status, id],
    )
    .map_err(|error| format!("Cập nhật trạng thái XML thất bại: {error}"))?;
    Ok(())
}

fn fail_file(db: &AppDb, id: i64, status: &str, message: &str) -> Result<(), String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    conn.execute("UPDATE xml_files SET status = ?1, error_message = ?2, updated_at = datetime('now') WHERE id = ?3", params![status, message, id])
        .map_err(|error| format!("Lưu lỗi XML thất bại: {error}"))?;
    Ok(())
}

fn save_hash(db: &AppDb, id: i64, hash: &str) -> Result<(), String> {
    update_value(db, "content_hash", id, hash)
}

fn save_request(db: &AppDb, id: i64, payload: &str) -> Result<(), String> {
    update_value(db, "request_payload", id, payload)
}

fn update_value(db: &AppDb, column: &str, id: i64, value: &str) -> Result<(), String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    conn.execute(
        &format!("UPDATE xml_files SET {column} = ?1, updated_at = datetime('now') WHERE id = ?2"),
        params![value, id],
    )
    .map_err(|error| format!("Cập nhật {column} thất bại: {error}"))?;
    Ok(())
}

fn save_patient(db: &AppDb, id: i64, code: &str, treatment_id: i64) -> Result<(), String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    conn.execute("UPDATE xml_files SET patient_code = ?1, nb_dot_dieu_tri_id = ?2, updated_at = datetime('now') WHERE id = ?3", params![code, treatment_id, id])
        .map_err(|error| format!("Lưu đối chiếu bệnh nhân thất bại: {error}"))?;
    Ok(())
}

fn duplicate_processed(db: &AppDb, id: i64, hash: &str) -> Result<bool, String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    let found = conn.query_row("SELECT 1 FROM xml_files WHERE content_hash = ?1 AND status = 'processed' AND id <> ?2 LIMIT 1", params![hash, id], |_| Ok(())).optional()
        .map_err(|error| format!("Kiểm tra XML trùng thất bại: {error}"))?;
    Ok(found.is_some())
}

fn mark_duplicate(db: &AppDb, id: i64, hash: &str) -> Result<(), String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    conn.execute("UPDATE xml_files SET status = 'processed', error_message = NULL, response_payload = ?1, processed_at = datetime('now'), updated_at = datetime('now') WHERE id = ?2", params![format!("duplicate_skipped:{hash}"), id])
        .map_err(|error| format!("Lưu XML trùng thất bại: {error}"))?;
    Ok(())
}

fn increment_attempt(db: &AppDb, id: i64) -> Result<(), String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    conn.execute("UPDATE xml_files SET attempt_count = attempt_count + 1, updated_at = datetime('now') WHERE id = ?1", params![id])
        .map_err(|error| format!("Tăng attempt_count thất bại: {error}"))?;
    Ok(())
}

fn finish_file(db: &AppDb, id: i64, response: &str) -> Result<(), String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    conn.execute("UPDATE xml_files SET status = 'processed', error_message = NULL, response_payload = ?1, processed_at = datetime('now'), updated_at = datetime('now') WHERE id = ?2", params![response, id])
        .map_err(|error| format!("Hoàn tất XML thất bại: {error}"))?;
    Ok(())
}

fn is_transient(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504)
}

fn preview(body: &str) -> String {
    body.chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_sample_refraction_to_expected_catalog_ids() {
        let catalog = catalog().expect("load catalog");
        assert_eq!(
            map_eye(
                catalog,
                &ParsedEye {
                    sphere: 1.75,
                    cylinder: -1.0,
                    axis: 178
                }
            )
            .expect("map right"),
            EyePayload {
                sph_id: 59,
                cyl_id: 176,
                ax_id: 391
            }
        );
        assert_eq!(
            map_eye(
                catalog,
                &ParsedEye {
                    sphere: 0.75,
                    cylinder: -0.25,
                    axis: 35
                }
            )
            .expect("map left"),
            EyePayload {
                sph_id: 55,
                cyl_id: 173,
                ax_id: 248
            }
        );
    }

    #[test]
    fn matches_only_one_treatment_for_patient_code() {
        let mut patients = PatientIndex::new();
        // Index lưu key đã lowercase (giống patient_index khi build từ API).
        patients.insert("hs001".into(), vec![Some(42)]);
        assert_eq!(
            match_treatment(&patients, " HS001 ").expect("unique patient"),
            42
        );
        // Khác hoa/thường vẫn khớp (XML ID vs maHoSo).
        patients.insert("hcm2607070269".into(), vec![Some(99)]);
        assert_eq!(
            match_treatment(&patients, "HCM2607070269").expect("case-insensitive"),
            99
        );
        assert_eq!(
            match_treatment(&patients, " hcm2607070269 ").expect("lower + trim"),
            99
        );

        patients.insert("dup".into(), vec![Some(1), Some(2)]);
        assert_eq!(
            match_treatment(&patients, "DUP").unwrap_err().0,
            "treatment_ambiguous"
        );
        assert_eq!(
            match_treatment(&patients, "MISSING").unwrap_err().0,
            "patient_not_found"
        );
    }

    #[test]
    fn normalize_patient_code_trims_and_lowercases() {
        assert_eq!(normalize_patient_code("  HCM2607070269 "), "hcm2607070269");
        assert_eq!(normalize_patient_code("abc"), "abc");
    }

    #[test]
    fn omits_new_glasses_when_copy_mode_is_disabled() {
        let eye = EyePayload {
            sph_id: 59,
            cyl_id: 176,
            ax_id: 391,
        };
        let payload = HisPayload {
            mat_phai_khuc_xa: eye.clone(),
            mat_trai_khuc_xa: eye,
            mat_phai_kinh_moi: None,
            mat_trai_kinh_moi: None,
        };
        let json = serde_json::to_value(payload).expect("serialize payload");
        assert!(json.get("matPhaiKinhMoi").is_none());
        assert!(json.get("matTraiKinhMoi").is_none());
    }
}
