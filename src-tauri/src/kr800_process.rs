use crate::app_logger;
use crate::db::AppDb;
use crate::his_api;
use crate::measurement_pair::{self, OrderedPair, PairResolve};
use crate::settings::{self, AppSettings};
use crate::xml_parser::{self, ParsedEye};
use crate::xml_track::{self, TrackedXmlFile};
use futures::stream::{self, StreamExt};
use reqwest::{Client, StatusCode};
use rusqlite::params;
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
/// Concurrency theo cặp / patient — không xử lý song song hai file cùng hồ sơ như hai PUT.
const MAX_CONCURRENT_PAIRS: usize = 5;
const MAX_TRANSIENT_RETRIES: u32 = 3;
const FILE_PROGRESS_EVENT: &str = "kr800:file-progress";
const PATIENT_LIST_EVENT: &str = "kr800:patient-list-ready";

#[derive(Default)]
pub struct Kr800ProcessState {
    run_lock: Mutex<()>,
    token_lock: Mutex<()>,
    patient_cache: Mutex<Option<PatientCache>>,
    /// JSON response API danh sách người bệnh (trong phiên app; không persist DB).
    last_patient_list: Mutex<Option<PatientListSnapshot>>,
    /// Khoá theo content hash (dedup) và patient_code_norm (pair).
    hash_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    patient_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

/// Snapshot response API người bệnh để UI xem JSON.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatientListSnapshot {
    pub body: String,
    pub from_time: String,
    pub to_time: String,
    pub patient_count: usize,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatientListReadyEvent {
    pub patient_count: usize,
    pub from_time: String,
    pub to_time: String,
    pub fetched_at: String,
}

#[derive(Clone)]
struct PatientCache {
    key: PatientCacheKey,
    index: Arc<PatientIndex>,
    raw_body: Arc<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatientCacheKey {
    from_time: String,
    to_time: String,
    api_url: String,
    username: String,
    query_fingerprint: String,
}

type PatientIndex = HashMap<String, Vec<Option<i64>>>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessResult {
    pub total: usize,
    pub processed: usize,
    pub failed: usize,
    pub skipped: usize,
    /// File lần 1 đang chờ lần đo 2 (không phải lỗi).
    pub awaiting_pair: usize,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct EyePayload {
    sph_id: i64,
    cyl_id: i64,
    ax_id: i64,
    /// Luôn serialize JSON null khi không có.
    don_vi_add_id: Option<i64>,
    thi_luc_id: Option<i64>,
}

/// Payload PUT sau khi ghép đủ hai lần đo KR-800.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct HisPayload {
    /// File lần 1 (sớm hơn) — R/Median
    mat_phai_kinh_sau_liet_dieu_tiet: EyePayload,
    /// File lần 1 — L/Median
    mat_trai_kinh_sau_liet_dieu_tiet: EyePayload,
    /// File lần 2 (muộn hơn) — R/Median
    mat_phai_kinh_truoc_liet_dieu_tiet: EyePayload,
    /// File lần 2 — L/Median
    mat_trai_kinh_truoc_liet_dieu_tiet: EyePayload,
}

struct WorkFile {
    id: i64,
    path: String,
}

#[derive(Default)]
struct FileOutcome {
    processed: bool,
    skipped: bool,
    awaiting_pair: bool,
    /// Thông báo info khi nhận lần đo 1 (không phải lỗi).
    info_message: Option<String>,
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
    state.patient_locks.lock().await.clear();
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
    let patients =
        patient_index(app, db, state, &client, &settings, from_time, to_time).await?;

    let waiting = waiting_files(db, from_time, to_time)?;
    let retry_pairs = measurement_pair::retryable_pairs(db)?;
    let total = waiting.len() + retry_pairs.len();

    // 1) Xử lý file waiting (parse + ghép cặp; PUT khi đủ hai lần đo).
    let file_outcomes = stream::iter(waiting.into_iter().map(|file| {
        process_one_file(app, db, state, &client, &settings, &patients, catalog, file)
    }))
    .buffer_unordered(MAX_CONCURRENT_PAIRS)
    .collect::<Vec<_>>()
    .await;

    // 2) Retry cặp đã đủ hai file nhưng gửi lỗi trước đó (cùng pair/payload).
    let pair_outcomes = stream::iter(retry_pairs.into_iter().map(|pair_id| {
        process_retry_pair(app, db, state, &client, &settings, &patients, catalog, pair_id)
    }))
    .buffer_unordered(MAX_CONCURRENT_PAIRS)
    .collect::<Vec<_>>()
    .await;

    let mut processed = 0usize;
    let mut skipped = 0usize;
    let mut awaiting_pair = 0usize;
    let mut info_notes = Vec::new();
    for outcome in file_outcomes.iter().chain(pair_outcomes.iter()) {
        if outcome.processed {
            processed += 1;
        }
        if outcome.skipped {
            skipped += 1;
        }
        if outcome.awaiting_pair {
            awaiting_pair += 1;
        }
        if let Some(msg) = &outcome.info_message {
            info_notes.push(msg.clone());
        }
    }
    let failed = total.saturating_sub(processed + skipped + awaiting_pair);
    let files = xml_track::list_xml_files(db, DEVICE_KEY, Some(from_time), Some(to_time))?;

    if !info_notes.is_empty() {
        for note in &info_notes {
            app_logger::info("kr800", note);
        }
    }

    Ok(ProcessResult {
        total,
        processed,
        failed,
        skipped,
        awaiting_pair,
        files,
    })
}

async fn process_one_file(
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
    match process_claimed_file(app, db, state, client, settings, patients, catalog, &file).await {
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

async fn process_claimed_file(
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

    let hash_lock = {
        let mut locks = state.hash_locks.lock().await;
        Arc::clone(
            locks
                .entry(hash.clone())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    };
    let _hash_guard = hash_lock.lock().await;

    if measurement_pair::content_hash_already_measured(db, &hash, file.id)
        .map_err(|error| ("xml_error", error))?
    {
        mark_duplicate(db, file.id, &hash).map_err(|error| ("xml_error", error))?;
        emit_file_progress(app, db, file.id);
        return Ok(FileOutcome {
            skipped: true,
            ..FileOutcome::default()
        });
    }

    let parsed = xml_parser::parse_measurement(&bytes).map_err(|error| ("xml_error", error))?;
    let meta = measurement_pair::meta_from_parsed(file.id, &parsed, &hash);
    measurement_pair::save_measurement_meta(db, &meta).map_err(|error| ("xml_error", error))?;
    set_stage(db, file.id, "parsed").map_err(|error| ("xml_error", error))?;
    emit_file_progress(app, db, file.id);

    let patient_lock = {
        let mut locks = state.patient_locks.lock().await;
        Arc::clone(
            locks
                .entry(meta.patient_code_norm.clone())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    };
    let _patient_guard = patient_lock.lock().await;

    // Re-check dedup sau khi lấy patient lock (race với file trùng).
    if measurement_pair::content_hash_already_measured(db, &hash, file.id)
        .map_err(|error| ("xml_error", error))?
    {
        mark_duplicate(db, file.id, &hash).map_err(|error| ("xml_error", error))?;
        emit_file_progress(app, db, file.id);
        return Ok(FileOutcome {
            skipped: true,
            ..FileOutcome::default()
        });
    }

    let resolve = measurement_pair::resolve_pair_for_measurement(db, &meta)
        .map_err(|error| ("xml_error", error))?;
    emit_file_progress(app, db, file.id);

    match resolve {
        PairResolve::AwaitingSecond {
            pair_id,
            patient_code,
        } => {
            let msg = format!(
                "Đã nhận lần đo 1 của {patient_code}, đang chờ lần đo 2. (pair_id={pair_id}, file_id={}, Patient.No.={}, measuredAt={})",
                meta.file_id, meta.patient_no, meta.measured_at
            );
            app_logger::info("kr800", &msg);
            emit_file_progress(app, db, file.id);
            Ok(FileOutcome {
                awaiting_pair: true,
                info_message: Some(msg),
                ..FileOutcome::default()
            })
        }
        PairResolve::PairingError { pair_id, message } => {
            app_logger::error(
                "kr800",
                &format!("pair_id={pair_id} pairing_error: {message}"),
            );
            emit_pair_progress(app, db, pair_id);
            Ok(FileOutcome::default())
        }
        PairResolve::ExtraMeasurement { message } => {
            app_logger::warn("kr800", &message);
            emit_file_progress(app, db, file.id);
            Ok(FileOutcome {
                skipped: true,
                info_message: Some(message),
                ..FileOutcome::default()
            })
        }
        PairResolve::Ready { pair_id, ordered } => {
            send_ready_pair(
                app, db, state, client, settings, patients, catalog, pair_id, &ordered,
            )
            .await
        }
    }
}

async fn process_retry_pair(
    app: &AppHandle,
    db: &AppDb,
    state: &Kr800ProcessState,
    client: &Client,
    settings: &AppSettings,
    patients: &PatientIndex,
    catalog: &Catalog,
    pair_id: i64,
) -> FileOutcome {
    let Some(pair) = measurement_pair::load_pair_by_id(db, pair_id).unwrap_or(None) else {
        return FileOutcome::default();
    };
    let (Some(id1), Some(id2)) = (pair.file_id_1, pair.file_id_2) else {
        return FileOutcome::default();
    };

    let patient_lock = {
        let mut locks = state.patient_locks.lock().await;
        Arc::clone(
            locks
                .entry(pair.patient_code_norm.clone())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    };
    let _patient_guard = patient_lock.lock().await;

    if !measurement_pair::claim_pair_for_send(db, pair_id).unwrap_or(false) {
        return FileOutcome::default();
    }
    emit_pair_progress(app, db, pair_id);

    // Ưu tiên payload đã lưu (retry không chọn lại file khác).
    let payload_json = if let Some(existing) = pair.request_payload.clone() {
        existing
    } else {
        match rebuild_payload_from_files(db, catalog, id1, id2) {
            Ok(json) => json,
            Err(error) => {
                let _ = measurement_pair::fail_pair(db, pair_id, "mapping_error", &error);
                emit_pair_progress(app, db, pair_id);
                return FileOutcome::default();
            }
        }
    };

    let treatment_id = match match_treatment(patients, &pair.patient_code) {
        Ok(id) => id,
        Err((status, message)) => {
            let _ = measurement_pair::fail_pair(db, pair_id, status, &message);
            emit_pair_progress(app, db, pair_id);
            return FileOutcome::default();
        }
    };

    let payload: HisPayload = match serde_json::from_str(&payload_json) {
        Ok(p) => p,
        Err(_) => {
            // payload cũ schema khác — rebuild.
            match rebuild_payload_from_files(db, catalog, id1, id2) {
                Ok(json) => match serde_json::from_str(&json) {
                    Ok(p) => p,
                    Err(error) => {
                        let _ = measurement_pair::fail_pair(
                            db,
                            pair_id,
                            "mapping_error",
                            &format!("Payload không hợp lệ: {error}"),
                        );
                        return FileOutcome::default();
                    }
                },
                Err(error) => {
                    let _ = measurement_pair::fail_pair(db, pair_id, "mapping_error", &error);
                    return FileOutcome::default();
                }
            }
        }
    };

    if let Err(error) =
        measurement_pair::save_pair_request(db, pair_id, treatment_id, &payload_json)
    {
        let _ = measurement_pair::fail_pair(db, pair_id, "mapping_error", &error);
        return FileOutcome::default();
    }

    match send_update(db, state, client, settings, treatment_id, &payload, &payload_json, pair_id)
        .await
    {
        Ok(response) => {
            let _ = measurement_pair::finish_pair_success(db, pair_id, &response);
            emit_pair_progress(app, db, pair_id);
            app_logger::info(
                "kr800",
                &format!(
                    "pair_id={pair_id} patient={} treatment_id={treatment_id} retry processed",
                    pair.patient_code
                ),
            );
            FileOutcome {
                processed: true,
                ..FileOutcome::default()
            }
        }
        Err(error) => {
            let _ = measurement_pair::fail_pair(db, pair_id, "send_error", &error);
            emit_pair_progress(app, db, pair_id);
            app_logger::error("kr800", &format!("pair_id={pair_id} send_error: {error}"));
            FileOutcome::default()
        }
    }
}

async fn send_ready_pair(
    app: &AppHandle,
    db: &AppDb,
    state: &Kr800ProcessState,
    client: &Client,
    settings: &AppSettings,
    patients: &PatientIndex,
    catalog: &Catalog,
    pair_id: i64,
    ordered: &OrderedPair,
) -> Result<FileOutcome, (&'static str, String)> {
    if !measurement_pair::claim_pair_for_send(db, pair_id).map_err(|e| ("send_error", e))? {
        // Task khác đã claim — không lỗi.
        return Ok(FileOutcome::default());
    }
    emit_pair_progress(app, db, pair_id);

    let first_eyes = load_parsed_eyes_from_disk(db, ordered.first.file_id)
        .await
        .map_err(|e| ("xml_error", e))?;
    let second_eyes = load_parsed_eyes_from_disk(db, ordered.second.file_id)
        .await
        .map_err(|e| ("xml_error", e))?;

    let payload = match build_his_payload(
        catalog,
        &first_eyes.0,
        &first_eyes.1,
        &second_eyes.0,
        &second_eyes.1,
    ) {
        Ok(p) => p,
        Err(error) => {
            let _ = measurement_pair::fail_pair(db, pair_id, "mapping_error", &error);
            emit_pair_progress(app, db, pair_id);
            return Ok(FileOutcome::default());
        }
    };
    let payload_json = match serde_json::to_string(&payload) {
        Ok(j) => j,
        Err(error) => {
            let msg = format!("Serialize request thất bại: {error}");
            let _ = measurement_pair::fail_pair(db, pair_id, "mapping_error", &msg);
            emit_pair_progress(app, db, pair_id);
            return Ok(FileOutcome::default());
        }
    };

    let treatment_id = match match_treatment(patients, &ordered.first.patient_code) {
        Ok(id) => id,
        Err((status, message)) => {
            let _ = measurement_pair::fail_pair(db, pair_id, status, &message);
            emit_pair_progress(app, db, pair_id);
            return Ok(FileOutcome::default());
        }
    };
    if let Err(e) = measurement_pair::save_pair_request(db, pair_id, treatment_id, &payload_json) {
        let _ = measurement_pair::fail_pair(db, pair_id, "mapping_error", &e);
        emit_pair_progress(app, db, pair_id);
        return Ok(FileOutcome::default());
    }
    emit_pair_progress(app, db, pair_id);

    app_logger::info(
        "kr800",
        &format!(
            "pair_id={pair_id} sending Patient.ID={} No1={} at1={} No2={} at2={} treatment_id={treatment_id}",
            ordered.first.patient_code,
            ordered.first.patient_no,
            ordered.first.measured_at,
            ordered.second.patient_no,
            ordered.second.measured_at,
        ),
    );

    match send_update(
        db,
        state,
        client,
        settings,
        treatment_id,
        &payload,
        &payload_json,
        pair_id,
    )
    .await
    {
        Ok(response) => {
            measurement_pair::finish_pair_success(db, pair_id, &response)
                .map_err(|e| ("send_error", e))?;
            emit_pair_progress(app, db, pair_id);
            app_logger::info(
                "kr800",
                &format!(
                    "pair_id={pair_id} files=({},{}) patient={} processed",
                    ordered.first.file_id, ordered.second.file_id, ordered.first.patient_code
                ),
            );
            Ok(FileOutcome {
                processed: true,
                ..FileOutcome::default()
            })
        }
        Err(error) => {
            // Giữ quan hệ cặp + status send_error trên cả pair và hai file (không fail_file đơn lẻ).
            let _ = measurement_pair::fail_pair(db, pair_id, "send_error", &error);
            emit_pair_progress(app, db, pair_id);
            app_logger::error("kr800", &format!("pair_id={pair_id} send_error: {error}"));
            Ok(FileOutcome::default())
        }
    }
}

async fn load_parsed_eyes_from_disk(
    db: &AppDb,
    file_id: i64,
) -> Result<(ParsedEye, ParsedEye), String> {
    let path: String = {
        let conn = db
            .conn
            .lock()
            .map_err(|_| "Không khóa được SQLite.".to_string())?;
        conn.query_row(
            "SELECT file_path FROM xml_files WHERE id = ?1",
            params![file_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Đọc path file id={file_id}: {e}"))?
    };
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("Không đọc XML id={file_id}: {e}"))?;
    let parsed = xml_parser::parse_measurement(&bytes)?;
    Ok((parsed.right, parsed.left))
}

fn rebuild_payload_from_files(
    db: &AppDb,
    catalog: &Catalog,
    file_id_1: i64,
    file_id_2: i64,
) -> Result<String, String> {
    let path1 = file_path(db, file_id_1)?;
    let path2 = file_path(db, file_id_2)?;
    let bytes1 = std::fs::read(&path1).map_err(|e| format!("Đọc file1: {e}"))?;
    let bytes2 = std::fs::read(&path2).map_err(|e| format!("Đọc file2: {e}"))?;
    let p1 = xml_parser::parse_measurement(&bytes1)?;
    let p2 = xml_parser::parse_measurement(&bytes2)?;
    // Đảm bảo order theo metadata (file_id_1/2 trên pair đã ordered).
    let payload = build_his_payload(catalog, &p1.right, &p1.left, &p2.right, &p2.left)?;
    serde_json::to_string(&payload).map_err(|e| format!("Serialize: {e}"))
}

fn file_path(db: &AppDb, file_id: i64) -> Result<String, String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    conn.query_row(
        "SELECT file_path FROM xml_files WHERE id = ?1",
        params![file_id],
        |row| row.get(0),
    )
    .map_err(|e| format!("Đọc path id={file_id}: {e}"))
}

fn build_his_payload(
    catalog: &Catalog,
    first_right: &ParsedEye,
    first_left: &ParsedEye,
    second_right: &ParsedEye,
    second_left: &ParsedEye,
) -> Result<HisPayload, String> {
    Ok(HisPayload {
        mat_phai_kinh_sau_liet_dieu_tiet: map_eye(catalog, first_right)?,
        mat_trai_kinh_sau_liet_dieu_tiet: map_eye(catalog, first_left)?,
        mat_phai_kinh_truoc_liet_dieu_tiet: map_eye(catalog, second_right)?,
        mat_trai_kinh_truoc_liet_dieu_tiet: map_eye(catalog, second_left)?,
    })
}

fn emit_pair_progress(app: &AppHandle, db: &AppDb, pair_id: i64) {
    let ids = {
        let Ok(conn) = db.conn.lock() else {
            return;
        };
        let mut stmt = match conn.prepare("SELECT id FROM xml_files WHERE pair_id = ?1") {
            Ok(s) => s,
            Err(_) => return,
        };
        let rows = match stmt.query_map(params![pair_id], |row| row.get::<_, i64>(0)) {
            Ok(r) => r,
            Err(_) => return,
        };
        rows.filter_map(|r| r.ok()).collect::<Vec<_>>()
    };
    for id in ids {
        emit_file_progress(app, db, id);
    }
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
    let key = measurement_pair::normalize_patient_code(patient_id);
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

pub async fn get_last_patient_list(state: &Kr800ProcessState) -> Option<PatientListSnapshot> {
    state.last_patient_list.lock().await.clone()
}

async fn patient_index(
    app: &AppHandle,
    db: &AppDb,
    state: &Kr800ProcessState,
    client: &Client,
    settings: &AppSettings,
    from_time: &str,
    to_time: &str,
) -> Result<Arc<PatientIndex>, String> {
    let stored_params = xml_track::get_patient_query_params(db, DEVICE_KEY)?;
    let query = build_patient_query(&stored_params, from_time, to_time);
    let key = PatientCacheKey {
        from_time: from_time.to_string(),
        to_time: to_time.to_string(),
        api_url: settings.his_api_url.trim().to_string(),
        username: settings.username.trim().to_string(),
        query_fingerprint: patient_query_fingerprint(&query),
    };
    if let Some(cache) = state.patient_cache.lock().await.as_ref() {
        if cache.key == key {
            let snapshot = PatientListSnapshot {
                body: (*cache.raw_body).clone(),
                from_time: from_time.to_string(),
                to_time: to_time.to_string(),
                patient_count: count_patients_in_index(&cache.index),
                fetched_at: chrono_now_local(),
            };
            store_and_emit_patient_list(app, state, snapshot).await;
            return Ok(Arc::clone(&cache.index));
        }
    }
    let url = his_api::join_url(&settings.his_api_url, PATIENT_PATH);
    let mut token = ensure_token(db, state).await?;
    let mut response = fetch_patients(client, &url, &token, &query).await?;
    if response.status() == StatusCode::UNAUTHORIZED {
        token = refresh_token(db, state, &token).await?;
        response = fetch_patients(client, &url, &token, &query).await?;
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
    let patient_count = envelope.data.len();
    let mut index: PatientIndex = HashMap::new();
    for patient in envelope.data {
        index
            .entry(measurement_pair::normalize_patient_code(&patient.ma_ho_so))
            .or_default()
            .push(patient.nb_dot_dieu_tri_id);
    }
    let raw_body = Arc::new(body);
    let index = Arc::new(index);
    *state.patient_cache.lock().await = Some(PatientCache {
        key,
        index: Arc::clone(&index),
        raw_body: Arc::clone(&raw_body),
    });
    let snapshot = PatientListSnapshot {
        body: (*raw_body).clone(),
        from_time: from_time.to_string(),
        to_time: to_time.to_string(),
        patient_count,
        fetched_at: chrono_now_local(),
    };
    store_and_emit_patient_list(app, state, snapshot).await;
    Ok(index)
}

async fn store_and_emit_patient_list(
    app: &AppHandle,
    state: &Kr800ProcessState,
    snapshot: PatientListSnapshot,
) {
    let event = PatientListReadyEvent {
        patient_count: snapshot.patient_count,
        from_time: snapshot.from_time.clone(),
        to_time: snapshot.to_time.clone(),
        fetched_at: snapshot.fetched_at.clone(),
    };
    *state.last_patient_list.lock().await = Some(snapshot);
    if let Err(error) = app.emit(PATIENT_LIST_EVENT, event) {
        app_logger::warn(
            "kr800",
            &format!("emit {PATIENT_LIST_EVENT} failed: {error}"),
        );
    }
}

fn count_patients_in_index(index: &PatientIndex) -> usize {
    index.values().map(|rows| rows.len()).sum()
}

fn chrono_now_local() -> String {
    chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn build_patient_query(
    params: &[xml_track::PatientQueryParam],
    from_time: &str,
    to_time: &str,
) -> Vec<(String, String)> {
    params
        .iter()
        .filter(|item| item.enabled && !item.key.trim().is_empty())
        .map(|item| {
            let key = item.key.trim().to_string();
            let value = match key.as_str() {
                "tuThoiGianVaoVien" => from_time.to_string(),
                "denThoiGianVaoVien" => to_time.to_string(),
                _ => item.value.clone(),
            };
            (key, value)
        })
        .collect()
}

fn patient_query_fingerprint(query: &[(String, String)]) -> String {
    query
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

async fn fetch_patients(
    client: &Client,
    url: &str,
    token: &str,
    query: &[(String, String)],
) -> Result<reqwest::Response, String> {
    client
        .get(url)
        .bearer_auth(token)
        .query(query)
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
    payload_json: &str,
    pair_id: i64,
) -> Result<String, String> {
    let url = format!(
        "{}/{}",
        his_api::join_url(&settings.his_api_url, UPDATE_PATH),
        treatment_id
    );
    app_logger::info(
        "kr800",
        &format!(
            "pair_id={} sending update treatment_id={} url={} body={}",
            pair_id, treatment_id, url, payload_json
        ),
    );
    let mut token = ensure_token(db, state).await?;
    let mut auth_retried = false;
    let mut transient_retries = 0u32;
    loop {
        measurement_pair::increment_pair_attempt(db, pair_id)?;
        let response = client
            .put(&url)
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
        don_vi_add_id: None,
        thi_luc_id: None,
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
        .prepare(
            r#"
            SELECT id, file_path FROM xml_files
            WHERE device_key = ?1
              AND status = 'waiting'
              AND created_at BETWEEN ?2 AND ?3
            ORDER BY created_at, id
            "#,
        )
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
        .execute(
            r#"
            UPDATE xml_files SET
              status = 'processing',
              error_message = NULL,
              attempt_count = 0,
              updated_at = datetime('now')
            WHERE id = ?1 AND status = 'waiting'
            "#,
            params![id],
        )
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
    conn.execute(
        "UPDATE xml_files SET status = ?1, error_message = ?2, updated_at = datetime('now') WHERE id = ?3",
        params![status, message, id],
    )
    .map_err(|error| format!("Lưu lỗi XML thất bại: {error}"))?;
    Ok(())
}

fn mark_duplicate(db: &AppDb, id: i64, hash: &str) -> Result<(), String> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| "Không khóa được SQLite.".to_string())?;
    conn.execute(
        r#"
        UPDATE xml_files SET
          status = 'processed',
          content_hash = ?1,
          error_message = NULL,
          response_payload = ?2,
          processed_at = datetime('now'),
          updated_at = datetime('now')
        WHERE id = ?3
        "#,
        params![hash, format!("duplicate_skipped:{hash}"), id],
    )
    .map_err(|error| format!("Lưu XML trùng thất bại: {error}"))?;
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
    use crate::xml_parser::ParsedEye;

    #[test]
    fn maps_sample_refraction_fixture_ids() {
        let catalog = catalog().expect("load catalog");
        // Fixture nghiệp vụ: SPH +0.25→53, CYL -1.00→176, AX 165→378
        assert_eq!(
            map_eye(
                catalog,
                &ParsedEye {
                    sphere: 0.25,
                    cylinder: -1.0,
                    axis: 165
                }
            )
            .expect("map right m1"),
            EyePayload {
                sph_id: 53,
                cyl_id: 176,
                ax_id: 378,
                don_vi_add_id: None,
                thi_luc_id: None,
            }
        );
        assert_eq!(
            map_eye(
                catalog,
                &ParsedEye {
                    sphere: 1.25,
                    cylinder: -1.75,
                    axis: 176
                }
            )
            .expect("map left m1"),
            EyePayload {
                sph_id: 57,
                cyl_id: 179,
                ax_id: 389,
                don_vi_add_id: None,
                thi_luc_id: None,
            }
        );
    }

    #[test]
    fn his_payload_has_four_fields_and_explicit_nulls() {
        let catalog = catalog().expect("catalog");
        let r1 = ParsedEye {
            sphere: 0.25,
            cylinder: -1.0,
            axis: 165,
        };
        let l1 = ParsedEye {
            sphere: 1.25,
            cylinder: -1.75,
            axis: 176,
        };
        // Lần 2: dùng giá trị khác trong catalog
        let r2 = ParsedEye {
            sphere: 1.75,
            cylinder: -1.0,
            axis: 178,
        };
        let l2 = ParsedEye {
            sphere: 0.75,
            cylinder: -0.25,
            axis: 35,
        };
        let payload = build_his_payload(catalog, &r1, &l1, &r2, &l2).expect("payload");
        let json = serde_json::to_value(&payload).expect("json");

        for key in [
            "matPhaiKinhSauLietDieuTiet",
            "matTraiKinhSauLietDieuTiet",
            "matPhaiKinhTruocLietDieuTiet",
            "matTraiKinhTruocLietDieuTiet",
        ] {
            assert!(json.get(key).is_some(), "missing {key}");
            let eye = &json[key];
            assert!(eye.get("donViAddId").unwrap().is_null());
            assert!(eye.get("thiLucId").unwrap().is_null());
        }
        // Không còn field cũ.
        assert!(json.get("matPhaiKhucXa").is_none());
        assert!(json.get("matTraiKhucXa").is_none());
        assert!(json.get("matPhaiKinhMoi").is_none());
        assert!(json.get("matTraiKinhMoi").is_none());

        assert_eq!(json["matPhaiKinhSauLietDieuTiet"]["sphId"], 53);
        assert_eq!(json["matPhaiKinhSauLietDieuTiet"]["cylId"], 176);
        assert_eq!(json["matPhaiKinhSauLietDieuTiet"]["axId"], 378);
        assert_eq!(json["matTraiKinhSauLietDieuTiet"]["sphId"], 57);
        assert_eq!(json["matTraiKinhSauLietDieuTiet"]["cylId"], 179);
        assert_eq!(json["matTraiKinhSauLietDieuTiet"]["axId"], 389);
    }

    #[test]
    fn matches_only_one_treatment_for_patient_code() {
        let mut patients = PatientIndex::new();
        patients.insert("hs001".into(), vec![Some(42)]);
        assert_eq!(
            match_treatment(&patients, " HS001 ").expect("unique patient"),
            42
        );
        patients.insert("hcm2607070269".into(), vec![Some(99)]);
        assert_eq!(
            match_treatment(&patients, "HCM2607070269").expect("case-insensitive"),
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
    fn maps_legacy_sample_refraction() {
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
            .expect("map right")
            .sph_id,
            59
        );
    }
}
