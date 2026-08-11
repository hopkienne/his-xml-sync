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
const TREATMENT_SUMMARY_PATH: &str = "/api/his/v1/nb-dot-dieu-tri/tong-hop";
/// Concurrency theo cặp / patient — không xử lý song song hai file cùng hồ sơ như hai PUT.
const MAX_CONCURRENT_PAIRS: usize = 5;
const MAX_TRANSIENT_RETRIES: u32 = 3;
const FILE_PROGRESS_EVENT: &str = "kr800:file-progress";
const PATIENT_LIST_EVENT: &str = "kr800:patient-list-ready";

pub struct Kr800ProcessState {
    run_lock: Mutex<()>,
    token_lock: Mutex<()>,
    patient_cache: Mutex<Option<PatientCache>>,
    /// JSON response API danh sách người bệnh (trong phiên app; không persist DB).
    last_patient_list: Mutex<Option<PatientListSnapshot>>,
    /// Khoá theo content hash (dedup) và patient_code_norm (pair).
    hash_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    patient_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// ID process app — gắn DB lease khi claim pair.
    pub instance_id: String,
}

impl Default for Kr800ProcessState {
    fn default() -> Self {
        Self {
            run_lock: Mutex::new(()),
            token_lock: Mutex::new(()),
            patient_cache: Mutex::new(None),
            last_patient_list: Mutex::new(None),
            hash_locks: Mutex::new(HashMap::new()),
            patient_locks: Mutex::new(HashMap::new()),
            instance_id: measurement_pair::generate_instance_id(),
        }
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    mat_phai_kinh_sau_liet_dieu_tiet: Option<EyePayload>,
    /// File lần 1 — L/Median
    #[serde(skip_serializing_if = "Option::is_none")]
    mat_trai_kinh_sau_liet_dieu_tiet: Option<EyePayload>,
    /// File lần 2 (muộn hơn) — R/Median
    #[serde(skip_serializing_if = "Option::is_none")]
    mat_phai_kinh_truoc_liet_dieu_tiet: Option<EyePayload>,
    /// File lần 2 — L/Median
    #[serde(skip_serializing_if = "Option::is_none")]
    mat_trai_kinh_truoc_liet_dieu_tiet: Option<EyePayload>,
}

#[derive(Debug, Deserialize)]
struct TreatmentSummaryEnvelope { data: TreatmentSummaryData }

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TreatmentSummaryData { ds_dv_kham: Vec<ServiceVisit> }

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceVisit { id: i64, nb_dot_dieu_tri_id: Option<i64> }

struct WorkFile {
    id: i64,
    file_name: String,
    path: String,
}

#[derive(Default)]
struct FileOutcome {
    processed: bool,
    skipped: bool,
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

    // Dưới run_lock: reclaim pair `sending` **hết lease** (không đụng lease còn sống).
    match measurement_pair::recover_expired_sending_files(db) {
        Ok(n) if n > 0 => {
            app_logger::warn(
                "kr800",
                &format!("recover_expired_sending: {n} pair(s) → send_error"),
            );
        }
        Ok(_) => {}
        Err(error) => {
            app_logger::error("kr800", &format!("recover_expired_sending failed: {error}"));
            return Err(error);
        }
    }

    match measurement_pair::reconcile_pending_patient_codes(db) {
        Ok(result) if result.reconciled_files > 0 || result.invalid_files > 0 => app_logger::info("kr800", &format!(
            "reconcile_filename_patient_codes reconciled_files={} invalid_files={}", result.reconciled_files, result.invalid_files
        )),
        Ok(_) => {}
        Err(error) => return Err(error),
    }

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

    // Queue trước: chỉ fetch patient-list range hiện tại khi có XML waiting.
    let waiting = waiting_files(db, from_time, to_time)?;
    let retry_pairs = measurement_pair::retryable_files(db)?;
    let total = waiting.len() + retry_pairs.len();

    // 1) XML waiting — dùng patient index theo from/to batch.
    let file_outcomes = stream::iter(waiting.into_iter().map(|file| {
        process_one_file(
            app,
            db,
            state,
            &client,
            &settings,
            catalog,
            file,
        )
    }))
    .buffer_unordered(MAX_CONCURRENT_PAIRS)
    .collect::<Vec<_>>()
    .await;

    // 2) Retry pair: treatment đã lưu hoặc tra theo measured_at_1/2 (không mặc định ngày batch).
    let pair_outcomes = stream::iter(retry_pairs.into_iter().map(|file_id| {
        process_retry_file(app, db, state, &client, &settings, catalog, file_id)
    }))
    .buffer_unordered(MAX_CONCURRENT_PAIRS)
    .collect::<Vec<_>>()
    .await;

    let mut processed = 0usize;
    let mut skipped = 0usize;
    let mut info_notes = Vec::new();
    for outcome in file_outcomes.iter().chain(pair_outcomes.iter()) {
        if outcome.processed {
            processed += 1;
        }
        if outcome.skipped {
            skipped += 1;
        }
        if let Some(msg) = &outcome.info_message {
            info_notes.push(msg.clone());
        }
    }
    // This is an informational pair count, deliberately independent of the
    // number of successful file PUTs.
    let awaiting_pair = measurement_pair::count_open_pairs(db)?;
    let failed = files_failed_in_scope(db, from_time, to_time)?;
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
    catalog: &Catalog,
    file: WorkFile,
) -> FileOutcome {
    match claim_file(db, file.id) {
        Ok(true) => {}
        Ok(false) => return FileOutcome::default(),
        Err(error) => {
            app_logger::error(
                "kr800",
                &format!("file_id={} claim_file failed: {error}", file.id),
            );
            return FileOutcome::default();
        }
    }
    emit_file_progress(app, db, file.id);
    match process_claimed_file(app, db, state, client, settings, catalog, &file).await {
        Ok(outcome) => outcome,
        Err((status, message)) => {
            // Lỗi trước khi pair claim — chỉ file hiện tại (chưa thuộc pair gửi).
            if let Err(e) = fail_file(db, file.id, status, &message) {
                app_logger::error(
                    "kr800",
                    &format!("file_id={} fail_file after error failed: {e}", file.id),
                );
            }
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
    catalog: &Catalog,
    file: &WorkFile,
) -> Result<FileOutcome, (&'static str, String)> {
    let filename_meta = xml_track::parse_kr800_filename(&file.file_name)
        .map_err(|error| ("invalid_filename", error))?;
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
    app_logger::info("kr800", &format!("file_id={} file_name={} ma_ho_so_from_filename={} xml_patient_id={}", file.id, file.file_name, filename_meta.ma_ho_so, parsed.xml_patient_id.as_deref().unwrap_or("<missing>")));
    if let Some(xml_patient_id) = parsed.xml_patient_id.as_deref() {
        if measurement_pair::normalize_patient_code(xml_patient_id) != measurement_pair::normalize_patient_code(&filename_meta.ma_ho_so) {
            app_logger::warn("kr800", &format!("KR-800 patient identifier mismatch: file_id={} maHoSoFromFilename={} xmlPatientId={} using=filename", file.id, filename_meta.ma_ho_so, xml_patient_id));
        }
    }
    let meta = measurement_pair::meta_from_parsed(file.id, &parsed, &hash, filename_meta.ma_ho_so);
    measurement_pair::save_measurement_meta(db, &meta, &parsed)
        .map_err(|error| ("xml_error", error))?;
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
            let mut outcome = send_measurement_file(app, db, state, client, settings, catalog, pair_id, file.id).await;
            if outcome.processed {
                outcome.info_message = Some(format!("Đã gửi lần đo 1, đang chờ lần đo 2 (pair_id={pair_id}, file_id={}, patient_code={patient_code}).", file.id));
            }
            Ok(outcome)
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
            Ok(send_measurement_file(app, db, state, client, settings, catalog, pair_id, ordered.second.file_id).await)
        }
    }
}

async fn process_retry_file(
    app: &AppHandle,
    db: &AppDb,
    state: &Kr800ProcessState,
    client: &Client,
    settings: &AppSettings,
    catalog: &Catalog,
    file_id: i64,
) -> FileOutcome {
    let file = match measurement_pair::load_file_send_record(db, file_id) {
        Ok(Some(file)) => file,
        Ok(None) => return FileOutcome::default(),
        Err(error) => { app_logger::error("kr800", &format!("file_id={file_id} retry load: {error}")); return FileOutcome::default(); }
    };
    send_measurement_file(app, db, state, client, settings, catalog, file.pair_id, file_id).await
}

async fn send_measurement_file(
    app: &AppHandle,
    db: &AppDb,
    state: &Kr800ProcessState,
    client: &Client,
    settings: &AppSettings,
    catalog: &Catalog,
    pair_id: i64,
    file_id: i64,
) -> FileOutcome {
    match measurement_pair::claim_file_for_send(db, file_id, &state.instance_id) {
        Ok(true) => {}
        Ok(false) => return FileOutcome::default(),
        Err(error) => { app_logger::error("kr800", &format!("pair_id={pair_id} file_id={file_id} claim: {error}")); return FileOutcome::default(); }
    }
    emit_file_progress(app, db, file_id);
    let result = async {
        let file = measurement_pair::load_file_send_record(db, file_id)?.ok_or_else(|| "File vừa claim không tồn tại.".to_string())?;
        let pair = measurement_pair::load_pair_by_id(db, pair_id)?.ok_or_else(|| "Pair vừa claim không tồn tại.".to_string())?;
        let expected = measurement_pair::expected_meta_for_order(&pair, file.pair_order)?;
        let snapshot = measurement_pair::load_or_rehydrate_snapshot(db, file_id, &expected)?;
        let (built_payload, payload_kind) = if file.pair_order == 1 {
            (build_first_measurement_payload(catalog, &snapshot.right_eye(), &snapshot.left_eye())?, "after_dilation")
        } else {
            (build_second_measurement_payload(catalog, &snapshot.right_eye(), &snapshot.left_eye())?, "before_dilation")
        };
        let (payload, payload_json) = match file.request_payload {
            Some(saved) => match serde_json::from_str::<HisPayload>(&saved) {
                Ok(payload) if payload_matches_order(&payload, file.pair_order) => (payload, saved),
                Ok(_) | Err(_) => {
                    let json = serde_json::to_string(&built_payload).map_err(|e| format!("Serialize payload: {e}"))?;
                    (built_payload, json)
                }
            },
            None => {
                let json = serde_json::to_string(&built_payload).map_err(|e| format!("Serialize payload: {e}"))?;
                (built_payload, json)
            }
        };
        let nb_id = match pair.nb_dot_dieu_tri_id.or(file.nb_dot_dieu_tri_id) {
            Some(id) => id,
            None => {
                let (from, to) = measurement_query_range_for_measurement(&expected.measured_at)?;
                let index = patient_index(app, db, state, client, settings, &from, &to).await?;
                match match_treatment_in_range(&index, &pair.patient_code, Some((&from, &to))) {
                    Ok(id) => id,
                    Err((status, message)) if status == "patient_not_found" => return Err(format!("{status}: {message} (source=filename, xmlPatientId={})", snapshot.xml_patient_id.as_deref().unwrap_or("<missing>"))),
                    Err((status, message)) => return Err(format!("{status}: {message}")),
                }
            }
        };
        let dv_id = match pair.dv_kham_id.or(file.dv_kham_id) {
            Some(id) => id,
            None => resolve_service_visit_id(db, state, client, settings, nb_id, pair_id, file_id).await?,
        };
        measurement_pair::save_file_request(db, file_id, pair_id, nb_id, dv_id, &payload_json, &state.instance_id)?;
        app_logger::info("kr800", &format!("pair_id={pair_id} file_id={file_id} pair_order={} patient_code={} nb_dot_dieu_tri_id={nb_id} dv_kham_id={dv_id} endpoint={UPDATE_PATH}/{dv_id} payload_kind={payload_kind} attempt={}", file.pair_order, pair.patient_code, file.attempt_count + 1));
        let response = send_file_update(db, state, client, settings, dv_id, &payload, pair_id, file_id).await?;
        Ok::<String, String>(response)
    }.await;
    match result {
        Ok(response) => match measurement_pair::finish_file_success(db, file_id, &response, &state.instance_id) {
            Ok(()) => { emit_pair_progress(app, db, pair_id); FileOutcome { processed: true, ..FileOutcome::default() } }
            Err(error) => { app_logger::error("kr800", &format!("pair_id={pair_id} file_id={file_id} HIS OK but DB finish failed: {error}")); FileOutcome::default() }
        },
        Err(message) => {
            // API/service lookup failures are retryable; the typed parser emits
            // the service-not-found distinction in its message/status below.
            let status = if message.starts_with("service_not_found:") { "service_not_found" }
                else if message.starts_with("patient_not_found:") { "patient_not_found" }
                else if message.starts_with("treatment_ambiguous:") { "treatment_ambiguous" }
                else if message.starts_with("mapping_error:") { "mapping_error" } else { "send_error" };
            let clean = message.split_once(':').map(|(_, text)| text.trim()).unwrap_or(&message);
            if let Err(error) = measurement_pair::fail_file_send(db, file_id, status, clean, &state.instance_id) {
                app_logger::error("kr800", &format!("file_id={file_id} fail send: {error}"));
            }
            emit_pair_progress(app, db, pair_id);
            app_logger::error("kr800", &format!("pair_id={pair_id} file_id={file_id} status={status}: {clean}"));
            FileOutcome::default()
        }
    }
}

async fn legacy_process_retry_pair(
    app: &AppHandle,
    db: &AppDb,
    state: &Kr800ProcessState,
    client: &Client,
    settings: &AppSettings,
    catalog: &Catalog,
    pair_id: i64,
) -> FileOutcome {
    let pair = match measurement_pair::load_pair_by_id(db, pair_id) {
        Ok(Some(p)) => p,
        Ok(None) => {
            app_logger::error("kr800", &format!("retry pair_id={pair_id}: không tồn tại"));
            return FileOutcome::default();
        }
        Err(error) => {
            app_logger::error("kr800", &format!("retry load pair_id={pair_id}: {error}"));
            return FileOutcome::default();
        }
    };
    let (Some(id1), Some(id2)) = (pair.file_id_1, pair.file_id_2) else {
        app_logger::error(
            "kr800",
            &format!("retry pair_id={pair_id}: thiếu file_id_1/2"),
        );
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

    match measurement_pair::claim_pair_for_send(db, pair_id, &state.instance_id) {
        Ok(true) => {}
        Ok(false) => return FileOutcome::default(),
        Err(error) => {
            app_logger::error(
                "kr800",
                &format!("pair_id={pair_id} claim_pair_for_send: {error}"),
            );
            // Claim chưa thành công → fail_pair với owner hiện tại thường no-op (CAS); log đủ.
            if let Err(e) =
                measurement_pair::fail_pair(db, pair_id, "send_error", &error, &state.instance_id)
            {
                app_logger::error("kr800", &format!("pair_id={pair_id} fail after claim err: {e}"));
            }
            return FileOutcome::default();
        }
    }
    emit_pair_progress(app, db, pair_id);

    // Ưu tiên payload đã lưu (cùng pair, không chọn file khác).
    let (payload, payload_json) = match resolve_payload_for_pair(db, catalog, &pair, id1, id2) {
        Ok(v) => v,
        Err(error) => {
            if let Err(e) =
                measurement_pair::fail_pair(db, pair_id, "mapping_error", &error, &state.instance_id)
            {
                app_logger::error("kr800", &format!("pair_id={pair_id} fail_pair: {e}"));
            }
            emit_pair_progress(app, db, pair_id);
            return FileOutcome::default();
        }
    };

    // Treatment: đã lưu → dùng luôn; chưa có → query theo measured_at_1/2 (không dùng ngày batch).
    let treatment_id = match resolve_treatment_for_retry_pair(
        app, db, state, client, settings, &pair,
    )
    .await
    {
        Ok(id) => id,
        Err((status, message)) => {
            if let Err(e) =
                measurement_pair::fail_pair(db, pair_id, status, &message, &state.instance_id)
            {
                app_logger::error("kr800", &format!("pair_id={pair_id} fail_pair: {e}"));
            }
            emit_pair_progress(app, db, pair_id);
            return FileOutcome::default();
        }
    };

    if let Err(error) =
        measurement_pair::save_pair_request(db, pair_id, treatment_id, &payload_json, &state.instance_id)
    {
        if let Err(e) = measurement_pair::fail_pair(db, pair_id, "send_error", &error, &state.instance_id) {
            app_logger::error("kr800", &format!("pair_id={pair_id} fail after save_request: {e}"));
        }
        emit_pair_progress(app, db, pair_id);
        return FileOutcome::default();
    }

    match send_update(db, state, client, settings, treatment_id, &payload, &payload_json, pair_id)
        .await
    {
        Ok(response) => match measurement_pair::finish_pair_success(db, pair_id, &response, &state.instance_id) {
            Ok(()) => {
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
                // HTTP OK nhưng DB chưa processed — không báo processed:true; đưa về retryable.
                app_logger::error(
                    "kr800",
                    &format!(
                        "pair_id={pair_id} HIS OK but finish_pair_success failed: {error}"
                    ),
                );
                let msg = format!("HIS đã nhận nhưng ghi DB processed thất bại: {error}");
                if let Err(e) = measurement_pair::fail_pair(db, pair_id, "send_error", &msg, &state.instance_id) {
                    app_logger::error(
                        "kr800",
                        &format!("pair_id={pair_id} fail after finish error: {e}"),
                    );
                }
                emit_pair_progress(app, db, pair_id);
                FileOutcome::default()
            }
        },
        Err(error) => {
            if let Err(e) = measurement_pair::fail_pair(db, pair_id, "send_error", &error, &state.instance_id) {
                app_logger::error("kr800", &format!("pair_id={pair_id} fail_pair send: {e}"));
            }
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
    // Mọi lỗi sau claim phải fail_pair (cả pair + 2 XML), không return Err để fail_file 1 XML.
    match measurement_pair::claim_pair_for_send(db, pair_id, &state.instance_id) {
        Ok(true) => {}
        Ok(false) => return Ok(FileOutcome::default()),
        Err(error) => {
            app_logger::error(
                "kr800",
                &format!("pair_id={pair_id} claim_pair_for_send: {error}"),
            );
            if let Err(e) = measurement_pair::fail_pair(db, pair_id, "send_error", &error, &state.instance_id) {
                app_logger::error("kr800", &format!("pair_id={pair_id} fail after claim: {e}"));
            }
            emit_pair_progress(app, db, pair_id);
            return Ok(FileOutcome::default());
        }
    }
    emit_pair_progress(app, db, pair_id);

    let eyes = match load_ordered_eyes_from_snapshots(db, ordered) {
        Ok(e) => e,
        Err(error) => {
            app_logger::error(
                "kr800",
                &format!("pair_id={pair_id} snapshot/load failed: {error}"),
            );
            // mapping_error: pair CHECK không có xml_error; integrity snapshot = lỗi mapping/gửi.
            if let Err(e) = measurement_pair::fail_pair(db, pair_id, "mapping_error", &error, &state.instance_id) {
                app_logger::error("kr800", &format!("pair_id={pair_id} fail_pair: {e}"));
            }
            emit_pair_progress(app, db, pair_id);
            return Ok(FileOutcome::default());
        }
    };

    let payload = match build_his_payload(
        catalog,
        &eyes.0,
        &eyes.1,
        &eyes.2,
        &eyes.3,
    ) {
        Ok(p) => p,
        Err(error) => {
            if let Err(e) = measurement_pair::fail_pair(db, pair_id, "mapping_error", &error, &state.instance_id) {
                app_logger::error("kr800", &format!("pair_id={pair_id} fail_pair: {e}"));
            }
            emit_pair_progress(app, db, pair_id);
            return Ok(FileOutcome::default());
        }
    };
    let payload_json = match serde_json::to_string(&payload) {
        Ok(j) => j,
        Err(error) => {
            let msg = format!("Serialize request thất bại: {error}");
            if let Err(e) = measurement_pair::fail_pair(db, pair_id, "mapping_error", &msg, &state.instance_id) {
                app_logger::error("kr800", &format!("pair_id={pair_id} fail_pair: {e}"));
            }
            emit_pair_progress(app, db, pair_id);
            return Ok(FileOutcome::default());
        }
    };

    let treatment_id = match match_treatment(patients, &ordered.first.patient_code) {
        Ok(id) => id,
        Err((status, message)) => {
            if let Err(e) = measurement_pair::fail_pair(db, pair_id, status, &message, &state.instance_id) {
                app_logger::error("kr800", &format!("pair_id={pair_id} fail_pair: {e}"));
            }
            emit_pair_progress(app, db, pair_id);
            return Ok(FileOutcome::default());
        }
    };
    if let Err(error) =
        measurement_pair::save_pair_request(db, pair_id, treatment_id, &payload_json, &state.instance_id)
    {
        if let Err(e) = measurement_pair::fail_pair(db, pair_id, "send_error", &error, &state.instance_id) {
            app_logger::error("kr800", &format!("pair_id={pair_id} fail after save: {e}"));
        }
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
        Ok(response) => match measurement_pair::finish_pair_success(db, pair_id, &response, &state.instance_id) {
            Ok(()) => {
                emit_pair_progress(app, db, pair_id);
                app_logger::info(
                    "kr800",
                    &format!(
                        "pair_id={pair_id} files=({},{}) patient={} processed",
                        ordered.first.file_id,
                        ordered.second.file_id,
                        ordered.first.patient_code
                    ),
                );
                Ok(FileOutcome {
                    processed: true,
                    ..FileOutcome::default()
                })
            }
            Err(error) => {
                app_logger::error(
                    "kr800",
                    &format!(
                        "pair_id={pair_id} HIS OK but finish_pair_success failed: {error}"
                    ),
                );
                let msg = format!("HIS đã nhận nhưng ghi DB processed thất bại: {error}");
                if let Err(e) = measurement_pair::fail_pair(db, pair_id, "send_error", &msg, &state.instance_id) {
                    app_logger::error(
                        "kr800",
                        &format!("pair_id={pair_id} fail after finish error: {e}"),
                    );
                }
                emit_pair_progress(app, db, pair_id);
                Ok(FileOutcome::default())
            }
        },
        Err(error) => {
            if let Err(e) = measurement_pair::fail_pair(db, pair_id, "send_error", &error, &state.instance_id) {
                app_logger::error("kr800", &format!("pair_id={pair_id} fail_pair: {e}"));
            }
            emit_pair_progress(app, db, pair_id);
            app_logger::error("kr800", &format!("pair_id={pair_id} send_error: {error}"));
            Ok(FileOutcome::default())
        }
    }
}

/// (R1, L1, R2, L2) từ snapshot DB — không đọc file mutable để PUT.
fn load_ordered_eyes_from_snapshots(
    db: &AppDb,
    ordered: &OrderedPair,
) -> Result<(ParsedEye, ParsedEye, ParsedEye, ParsedEye), String> {
    let exp1 = measurement_pair::ExpectedFileMeta {
        content_hash: ordered.first.content_hash.clone(),
        patient_code: ordered.first.patient_code.clone(),
        patient_no: ordered.first.patient_no,
        measured_at: ordered.first.measured_at.clone(),
    };
    let exp2 = measurement_pair::ExpectedFileMeta {
        content_hash: ordered.second.content_hash.clone(),
        patient_code: ordered.second.patient_code.clone(),
        patient_no: ordered.second.patient_no,
        measured_at: ordered.second.measured_at.clone(),
    };
    let s1 = measurement_pair::load_or_rehydrate_snapshot(db, ordered.first.file_id, &exp1)?;
    let s2 = measurement_pair::load_or_rehydrate_snapshot(db, ordered.second.file_id, &exp2)?;
    Ok((
        s1.right_eye(),
        s1.left_eye(),
        s2.right_eye(),
        s2.left_eye(),
    ))
}

/// Resolve payload retry: ưu tiên request_payload đã lưu; rebuild chỉ từ snapshot hợp lệ.
fn resolve_payload_for_pair(
    db: &AppDb,
    catalog: &Catalog,
    pair: &measurement_pair::PairRecord,
    id1: i64,
    id2: i64,
) -> Result<(HisPayload, String), String> {
    if let Some(existing) = pair.request_payload.as_ref() {
        if let Ok(payload) = serde_json::from_str::<HisPayload>(existing) {
            return Ok((payload, existing.clone()));
        }
        // JSON cũ hỏng / schema khác — rebuild từ snapshot, lưu JSON mới (không giữ chuỗi hỏng).
        app_logger::warn(
            "kr800",
            &format!(
                "pair_id={} request_payload không deserialize được — rebuild từ snapshot",
                pair.id
            ),
        );
    }

    let exp1 = measurement_pair::expected_meta_for_order(pair, 1)?;
    let exp2 = measurement_pair::expected_meta_for_order(pair, 2)?;
    let s1 = measurement_pair::load_or_rehydrate_snapshot(db, id1, &exp1)?;
    let s2 = measurement_pair::load_or_rehydrate_snapshot(db, id2, &exp2)?;
    let payload = build_his_payload(
        catalog,
        &s1.right_eye(),
        &s1.left_eye(),
        &s2.right_eye(),
        &s2.left_eye(),
    )?;
    let json = serde_json::to_string(&payload).map_err(|e| format!("Serialize payload: {e}"))?;
    Ok((payload, json))
}

fn build_his_payload(
    catalog: &Catalog,
    first_right: &ParsedEye,
    first_left: &ParsedEye,
    second_right: &ParsedEye,
    second_left: &ParsedEye,
) -> Result<HisPayload, String> {
    Ok(HisPayload {
        mat_phai_kinh_sau_liet_dieu_tiet: Some(map_eye(catalog, first_right)?),
        mat_trai_kinh_sau_liet_dieu_tiet: Some(map_eye(catalog, first_left)?),
        mat_phai_kinh_truoc_liet_dieu_tiet: Some(map_eye(catalog, second_right)?),
        mat_trai_kinh_truoc_liet_dieu_tiet: Some(map_eye(catalog, second_left)?),
    })
}

fn build_first_measurement_payload(catalog: &Catalog, right: &ParsedEye, left: &ParsedEye) -> Result<HisPayload, String> {
    Ok(HisPayload {
        mat_phai_kinh_sau_liet_dieu_tiet: Some(map_eye(catalog, right)?),
        mat_trai_kinh_sau_liet_dieu_tiet: Some(map_eye(catalog, left)?),
        mat_phai_kinh_truoc_liet_dieu_tiet: None,
        mat_trai_kinh_truoc_liet_dieu_tiet: None,
    })
}

fn build_second_measurement_payload(catalog: &Catalog, right: &ParsedEye, left: &ParsedEye) -> Result<HisPayload, String> {
    Ok(HisPayload {
        mat_phai_kinh_sau_liet_dieu_tiet: None,
        mat_trai_kinh_sau_liet_dieu_tiet: None,
        mat_phai_kinh_truoc_liet_dieu_tiet: Some(map_eye(catalog, right)?),
        mat_trai_kinh_truoc_liet_dieu_tiet: Some(map_eye(catalog, left)?),
    })
}

fn payload_matches_order(payload: &HisPayload, order: u8) -> bool {
    match order {
        1 => payload.mat_phai_kinh_sau_liet_dieu_tiet.is_some()
            && payload.mat_trai_kinh_sau_liet_dieu_tiet.is_some()
            && payload.mat_phai_kinh_truoc_liet_dieu_tiet.is_none()
            && payload.mat_trai_kinh_truoc_liet_dieu_tiet.is_none(),
        2 => payload.mat_phai_kinh_sau_liet_dieu_tiet.is_none()
            && payload.mat_trai_kinh_sau_liet_dieu_tiet.is_none()
            && payload.mat_phai_kinh_truoc_liet_dieu_tiet.is_some()
            && payload.mat_trai_kinh_truoc_liet_dieu_tiet.is_some(),
        _ => false,
    }
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
    match_treatment_in_range(patients, patient_id, None)
}

fn match_treatment_in_range(
    patients: &PatientIndex,
    patient_id: &str,
    query_range: Option<(&str, &str)>,
) -> Result<i64, (&'static str, String)> {
    let key = measurement_pair::normalize_patient_code(patient_id);
    let range_note = query_range
        .map(|(from, to)| format!(" (query {from} → {to})"))
        .unwrap_or_default();
    match patients.get(&key) {
        None => Err((
            "patient_not_found",
            format!("Không tìm thấy bệnh nhân có mã hồ sơ {patient_id}{range_note}."),
        )),
        Some(values) if values.len() > 1 => Err((
            "treatment_ambiguous",
            format!(
                "Tìm thấy {} đợt điều trị cho mã hồ sơ {}{range_note}.",
                values.len(),
                patient_id
            ),
        )),
        Some(values) if values.is_empty() => Err((
            "treatment_ambiguous",
            format!("Bệnh nhân {patient_id} không có đợt điều trị{range_note}."),
        )),
        Some(values) => values[0].ok_or_else(|| {
            (
                "treatment_ambiguous",
                format!("Bệnh nhân {patient_id} không có nbDotDieuTriId{range_note}."),
            )
        }),
    }
}

/// Khoảng patient-list từ measured_at_1/2: min day 00:00:00 → max day 23:59:59.
/// Không fallback sang ngày hiện tại nếu thiếu/sai format.
pub(crate) fn measurement_query_range_for_pair(
    pair: &measurement_pair::PairRecord,
) -> Result<(String, String), String> {
    let at1 = pair.measured_at_1.as_deref().filter(|s| !s.trim().is_empty());
    let at2 = pair.measured_at_2.as_deref().filter(|s| !s.trim().is_empty());
    let (raw1, raw2) = match (at1, at2) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            return Err(format!(
                "pair_id={} thiếu measured_at_1/2 (có {:?}, {:?}) — không fallback ngày hiện tại.",
                pair.id, pair.measured_at_1, pair.measured_at_2
            ));
        }
    };
    let dt1 = chrono::NaiveDateTime::parse_from_str(raw1, "%Y-%m-%d %H:%M:%S").map_err(|_| {
        format!(
            "pair_id={} measured_at_1 không parse được: {raw1}",
            pair.id
        )
    })?;
    let dt2 = chrono::NaiveDateTime::parse_from_str(raw2, "%Y-%m-%d %H:%M:%S").map_err(|_| {
        format!(
            "pair_id={} measured_at_2 không parse được: {raw2}",
            pair.id
        )
    })?;
    let d_min = dt1.date().min(dt2.date());
    let d_max = dt1.date().max(dt2.date());
    let from = chrono::NaiveDateTime::new(
        d_min,
        chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap_or(chrono::NaiveTime::MIN),
    );
    let to = chrono::NaiveDateTime::new(
        d_max,
        chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap_or(chrono::NaiveTime::MIN),
    );
    Ok((
        from.format("%Y-%m-%d %H:%M:%S").to_string(),
        to.format("%Y-%m-%d %H:%M:%S").to_string(),
    ))
}

/// Treatment cho retry: ưu tiên ID đã lưu; nếu thiếu → patient-list theo ngày đo pair.
async fn resolve_treatment_for_retry_pair(
    app: &AppHandle,
    db: &AppDb,
    state: &Kr800ProcessState,
    client: &Client,
    settings: &AppSettings,
    pair: &measurement_pair::PairRecord,
) -> Result<i64, (&'static str, String)> {
    if let Some(tid) = pair.nb_dot_dieu_tri_id {
        app_logger::info(
            "kr800",
            &format!(
                "pair_id={} retry dùng nb_dot_dieu_tri_id đã lưu={tid} (không fetch patient-list ngày hiện tại)",
                pair.id
            ),
        );
        return Ok(tid);
    }

    let (from, to) = measurement_query_range_for_pair(pair).map_err(|e| ("mapping_error", e))?;
    app_logger::info(
        "kr800",
        &format!(
            "pair_id={} retry thiếu treatment — patient-list theo đo {} → {}",
            pair.id, from, to
        ),
    );
    let index = patient_index(app, db, state, client, settings, &from, &to)
        .await
        .map_err(|e| ("patient_not_found", e))?;
    match_treatment_in_range(&index, &pair.patient_code, Some((&from, &to)))
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

fn measurement_query_range_for_measurement(measured_at: &str) -> Result<(String, String), String> {
    let parsed = chrono::NaiveDateTime::parse_from_str(measured_at, "%Y-%m-%d %H:%M:%S")
        .map_err(|_| format!("mapping_error: measured_at không hợp lệ: {measured_at}"))?;
    let day = parsed.date();
    Ok((
        chrono::NaiveDateTime::new(day, chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap()).format("%Y-%m-%d %H:%M:%S").to_string(),
        chrono::NaiveDateTime::new(day, chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap()).format("%Y-%m-%d %H:%M:%S").to_string(),
    ))
}

/// Deserialize the HIS wrapper at the boundary.  The service must belong to
/// the same treatment queried; a mismatched response is unsafe to PUT.
fn parse_service_visit_id(body: &str, expected_nb_id: i64) -> Result<i64, String> {
    let envelope: TreatmentSummaryEnvelope = serde_json::from_str(body)
        .map_err(|e| format!("Response dịch vụ khám không hợp lệ: {e}"))?;
    let service = envelope.data.ds_dv_kham.into_iter().next()
        .ok_or_else(|| "service_not_found: Không tìm thấy dịch vụ khám (dsDvKham rỗng).".to_string())?;
    match service.nb_dot_dieu_tri_id {
        Some(id) if id == expected_nb_id => Ok(service.id),
        Some(id) => Err(format!("service_not_found: Dữ liệu dịch vụ không nhất quán: nbDotDieuTriId={id}, expected={expected_nb_id}.")),
        None => Err("service_not_found: Dịch vụ khám thiếu nbDotDieuTriId.".into()),
    }
}

async fn resolve_service_visit_id(
    db: &AppDb,
    state: &Kr800ProcessState,
    client: &Client,
    settings: &AppSettings,
    nb_id: i64,
    pair_id: i64,
    file_id: i64,
) -> Result<i64, String> {
    let url = his_api::join_url(&settings.his_api_url, TREATMENT_SUMMARY_PATH);
    let query = [
        ("nbThongTinId", nb_id.to_string()), ("page", "0".into()),
        ("sort", "thoiGianVaoVien,desc".into()), ("size", "500".into()),
        ("active", "true".into()), ("dsCoSoKcbId", settings.ds_co_so_kcb_id.to_string()),
    ];
    let mut token = ensure_token(db, state).await?;
    let mut retried_auth = false;
    loop {
        let response = client.get(&url).bearer_auth(&token).query(&query).send().await
            .map_err(|e| {
                app_logger::error("kr800", &format!("pair_id={pair_id} file_id={file_id} nb_dot_dieu_tri_id={nb_id} endpoint={url} api=tong-hop request_error={e}"));
                format!("Gọi API dịch vụ khám thất bại: {e}")
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|e| format!("Đọc response dịch vụ khám thất bại: {e}"))?;
        let log = format!("pair_id={pair_id} file_id={file_id} nb_dot_dieu_tri_id={nb_id} endpoint={url} api=tong-hop response_status={status} response_body={}", response_log_body(&body));
        if status.is_success() { app_logger::info("kr800", &log); } else { app_logger::error("kr800", &log); }
        if status.is_success() { return parse_service_visit_id(&body, nb_id); }
        if status == StatusCode::UNAUTHORIZED && !retried_auth {
            retried_auth = true;
            token = refresh_token(db, state, &token).await?;
            continue;
        }
        return Err(format!("API dịch vụ khám trả về {status}: {}", preview(&body)));
    }
}

async fn send_file_update(
    db: &AppDb, state: &Kr800ProcessState, client: &Client, settings: &AppSettings,
    dv_kham_id: i64, payload: &HisPayload, pair_id: i64, file_id: i64,
) -> Result<String, String> {
    let url = format!("{}/{}", his_api::join_url(&settings.his_api_url, UPDATE_PATH), dv_kham_id);
    let mut token = ensure_token(db, state).await?;
    let mut auth_retried = false;
    let mut transient_retries = 0u32;
    loop {
        measurement_pair::increment_file_attempt(db, file_id, &state.instance_id)?;
        match client.put(&url).bearer_auth(&token).json(payload).send().await {
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.map_err(|e| format!("Không đọc được response HIS: {e}"))?;
                let log = format!("pair_id={pair_id} file_id={file_id} dv_kham_id={dv_kham_id} endpoint={url} api=nb-kham-ck-mat response_status={status} response_body={}", response_log_body(&body));
                if status.is_success() { app_logger::info("kr800", &log); } else { app_logger::error("kr800", &log); }
                if status.is_success() { return Ok(body); }
                if status == StatusCode::UNAUTHORIZED && !auth_retried { auth_retried = true; token = refresh_token(db, state, &token).await?; continue; }
                if is_transient(status) && transient_retries < MAX_TRANSIENT_RETRIES { sleep(Duration::from_secs(1 << transient_retries)).await; transient_retries += 1; continue; }
                return Err(format!("HIS trả về {status}: {}", preview(&body)));
            }
            Err(error) if !error.is_builder() && transient_retries < MAX_TRANSIENT_RETRIES => { sleep(Duration::from_secs(1 << transient_retries)).await; transient_retries += 1; }
            Err(error) => {
                app_logger::error("kr800", &format!("pair_id={pair_id} file_id={file_id} dv_kham_id={dv_kham_id} endpoint={url} api=nb-kham-ck-mat request_error={error}"));
                return Err(format!("Gửi HIS thất bại: {error}"));
            }
        }
    }
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
        measurement_pair::increment_pair_attempt(db, pair_id, &state.instance_id)?;
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
            SELECT id, file_name, file_path FROM xml_files
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
                file_name: row.get(1)?,
                path: row.get(2)?,
            })
        })
        .map_err(|error| format!("Đọc queue XML thất bại: {error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Map queue XML thất bại: {error}"))
}

fn files_failed_in_scope(db: &AppDb, from_time: &str, to_time: &str) -> Result<usize, String> {
    let conn = db.conn.lock().map_err(|_| "Không khóa được SQLite.".to_string())?;
    conn.query_row(r#"
        SELECT COUNT(*) FROM xml_files WHERE device_key=?1 AND created_at BETWEEN ?2 AND ?3
          AND status IN ('xml_error','mapping_error','send_error','patient_not_found','treatment_ambiguous','service_not_found','pairing_error','failed','invalid_filename')
    "#, params![DEVICE_KEY, from_time, to_time], |r| r.get::<_, i64>(0))
        .map(|n| n as usize).map_err(|e| format!("Đếm file lỗi: {e}"))
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

/// Response bodies are useful when reconciling HIS behavior with local state,
/// but a bounded value protects the rotating application log from oversized
/// error pages. `app_logger` still redacts token/password-shaped values.
fn response_log_body(body: &str) -> String {
    const MAX_RESPONSE_LOG_CHARS: usize = 16_000;
    let value: String = body.chars().take(MAX_RESPONSE_LOG_CHARS).collect();
    if body.chars().count() > MAX_RESPONSE_LOG_CHARS {
        format!("{value}… [truncated]")
    } else {
        value
    }
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
    fn first_and_second_payloads_omit_the_other_measurement_keys() {
        let catalog = catalog().expect("catalog");
        let eye = ParsedEye { sphere: 0.25, cylinder: -1.0, axis: 165 };
        let first = serde_json::to_value(build_first_measurement_payload(catalog, &eye, &eye).unwrap()).unwrap();
        assert!(first.get("matPhaiKinhSauLietDieuTiet").is_some());
        assert!(first.get("matTraiKinhSauLietDieuTiet").is_some());
        assert!(first.get("matPhaiKinhTruocLietDieuTiet").is_none());
        assert!(first.get("matTraiKinhTruocLietDieuTiet").is_none());
        assert!(first["matPhaiKinhSauLietDieuTiet"]["donViAddId"].is_null());
        let second = serde_json::to_value(build_second_measurement_payload(catalog, &eye, &eye).unwrap()).unwrap();
        assert!(second.get("matPhaiKinhSauLietDieuTiet").is_none());
        assert!(second.get("matTraiKinhSauLietDieuTiet").is_none());
        assert!(second.get("matPhaiKinhTruocLietDieuTiet").is_some());
        assert!(second.get("matTraiKinhTruocLietDieuTiet").is_some());
    }

    #[test]
    fn parses_first_service_and_rejects_missing_or_mismatched_treatment() {
        let body = r#"{"data":{"dsDvKham":[{"id":3462,"nbDotDieuTriId":1103}]}}"#;
        assert_eq!(parse_service_visit_id(body, 1103).unwrap(), 3462);
        assert!(parse_service_visit_id(r#"{"data":{"dsDvKham":[]}}"#, 1103).unwrap_err().contains("service_not_found"));
        assert!(parse_service_visit_id(r#"{"data":{"dsDvKham":[{"id":3462,"nbDotDieuTriId":99}]}}"#, 1103).unwrap_err().contains("không nhất quán"));
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

    #[test]
    fn measurement_query_range_covers_midnight_span() {
        let pair = measurement_pair::PairRecord {
            id: 1,
            patient_code: "HCM1".into(),
            patient_code_norm: "hcm1".into(),
            file_id_1: Some(1),
            file_id_2: Some(2),
            content_hash_1: Some("a".into()),
            content_hash_2: Some("b".into()),
            patient_no_1: Some(1),
            patient_no_2: Some(2),
            measured_at_1: Some("2026-07-15 23:58:00".into()),
            measured_at_2: Some("2026-07-16 00:05:00".into()),
            status: "send_error".into(),
            request_payload: None,
            nb_dot_dieu_tri_id: None,
            dv_kham_id: None,
        };
        let (from, to) = measurement_query_range_for_pair(&pair).expect("range");
        assert_eq!(from, "2026-07-15 00:00:00");
        assert_eq!(to, "2026-07-16 23:59:59");
    }

    #[test]
    fn measurement_query_range_rejects_missing_times() {
        let pair = measurement_pair::PairRecord {
            id: 9,
            patient_code: "X".into(),
            patient_code_norm: "x".into(),
            file_id_1: Some(1),
            file_id_2: Some(2),
            content_hash_1: None,
            content_hash_2: None,
            patient_no_1: None,
            patient_no_2: None,
            measured_at_1: None,
            measured_at_2: Some("2026-07-16 00:05:00".into()),
            status: "send_error".into(),
            request_payload: None,
            nb_dot_dieu_tri_id: None,
            dv_kham_id: None,
        };
        let err = measurement_query_range_for_pair(&pair).unwrap_err();
        assert!(err.contains("thiếu measured_at"), "{err}");
        assert!(!err.contains("2026-07-24"), "must not fallback to today");
    }
}
