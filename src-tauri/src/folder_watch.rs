//! Tự động theo dõi folder tracking KR-800:
//! - Poll định kỳ + FS watcher (`notify`) phát hiện XML mới
//! - Chỉ INSERT path chưa có trong DB
//! - Emit event cho UI refresh theo khoảng ngày
//! - Tự xử lý file `waiting` (auto process HIS) khi có file mới / còn chờ

use crate::app_logger;
use crate::db::AppDb;
use crate::kr800_process::{self, Kr800ProcessState, ProcessResult};
use crate::settings;
use crate::xml_track::{self, InsertedXmlFile, InsertNewResult};
use chrono::{Local, NaiveDate, NaiveDateTime, NaiveTime};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

const DEVICE_KEY: &str = "kr-800";
/// Poll an toàn cho share/PACS (bù event watcher bị miss).
const POLL_INTERVAL: Duration = Duration::from_secs(20);
/// Debounce sau khi có event FS trước khi insert.
const FS_DEBOUNCE: Duration = Duration::from_millis(1500);
/// File phải ổn định (mtime) trước khi index / process.
const MIN_FILE_AGE: Duration = Duration::from_secs(2);
/// Mỗi N tick poll (~60s) mới retry waiting còn sót (tránh spam HIS).
const PENDING_PROCESS_EVERY_N_POLLS: u32 = 3;

const EVENT_INDEXED: &str = "kr800:files-indexed";
const EVENT_AUTO_PROCESS: &str = "kr800:auto-process";
const EVENT_WATCH_STATUS: &str = "kr800:watch-status";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesIndexedEvent {
    pub source: String,
    pub inserted_count: usize,
    pub scanned_count: usize,
    pub tracking_folder: String,
    pub inserted: Vec<InsertedXmlFile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoProcessEvent {
    pub ok: bool,
    pub message: String,
    pub from_time: String,
    pub to_time: String,
    pub total: usize,
    pub processed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub busy: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchStatusEvent {
    pub active: bool,
    pub tracking_folder: Option<String>,
    pub message: String,
}

/// Khởi động vòng lặp nền (gọi một lần trong `setup`).
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        app_logger::info("folder_watch", "background watcher started");
        run_loop(app).await;
    });
}

async fn run_loop(app: AppHandle) {
    let mut last_folder: Option<String> = None;
    // Giữ watcher sống trong scope (drop = ngừng watch). Prefix `_` tránh warning unused.
    let mut _fs_watcher: Option<RecommendedWatcher> = None;
    let (fs_tx, mut fs_rx) = mpsc::unbounded_channel::<PathBuf>();
    let mut pending_poll_ticks: u32 = 0;

    let mut poll = tokio::time::interval(POLL_INTERVAL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    poll.tick().await;

    loop {
        tokio::select! {
            _ = poll.tick() => {
                if let Some(folder) = current_folder(&app) {
                    if last_folder.as_deref() != Some(folder.as_str()) {
                        last_folder = Some(folder.clone());
                        _fs_watcher = start_fs_watcher(&folder, fs_tx.clone());
                        let auto_on = auto_process_enabled(&app);
                        emit_watch_status(
                            &app,
                            true,
                            Some(folder.clone()),
                            if auto_on {
                                "Đang theo dõi folder (tự quét nền + tự ghép cặp / gửi HIS)."
                            } else {
                                "Đang theo dõi folder (tự quét nền). Tự xử lý HIS đang TẮT."
                            },
                        );
                        app_logger::info(
                            "folder_watch",
                            &format!("watching folder={folder}"),
                        );
                    }

                    match insert_new_poll_blocking(&app).await {
                        Ok(insert) if insert.inserted_count > 0 => {
                            pending_poll_ticks = 0;
                            emit_indexed(&app, "auto", &insert);
                            auto_process_after_insert(&app, &insert.inserted).await;
                        }
                        Ok(_) => {
                            pending_poll_ticks = pending_poll_ticks.saturating_add(1);
                            if pending_poll_ticks >= PENDING_PROCESS_EVERY_N_POLLS {
                                pending_poll_ticks = 0;
                                auto_process_pending_waiting(&app).await;
                            }
                        }
                        Err(err) => {
                            app_logger::error(
                                "folder_watch",
                                &format!("insert_new failed: {err}"),
                            );
                        }
                    }
                } else if last_folder.is_some() {
                    last_folder = None;
                    _fs_watcher = None;
                    pending_poll_ticks = 0;
                    emit_watch_status(
                        &app,
                        false,
                        None,
                        "Chưa chọn thư mục tracking — tạm dừng theo dõi nền.",
                    );
                }
            }
            Some(path) = fs_rx.recv() => {
                let mut batch = vec![path];
                // Debounce: gom event burst khi máy xuất nhiều file.
                tokio::time::sleep(FS_DEBOUNCE).await;
                while let Ok(p) = fs_rx.try_recv() {
                    batch.push(p);
                }
                match insert_paths_blocking(&app, batch).await {
                    Ok(insert) if insert.inserted_count > 0 => {
                        pending_poll_ticks = 0;
                        emit_indexed(&app, "watcher", &insert);
                        auto_process_after_insert(&app, &insert.inserted).await;
                    }
                    Ok(_) => {}
                    Err(err) => {
                        app_logger::error(
                            "folder_watch",
                            &format!("watcher insert failed: {err}"),
                        );
                    }
                }
            }
        }
    }
}

fn current_folder(app: &AppHandle) -> Option<String> {
    let db = app.try_state::<AppDb>()?;
    xml_track::get_device_folder(&db, DEVICE_KEY)
        .ok()
        .and_then(|s| s.tracking_folder)
        .filter(|f| !f.trim().is_empty())
}

fn start_fs_watcher(
    folder: &str,
    tx: mpsc::UnboundedSender<PathBuf>,
) -> Option<RecommendedWatcher> {
    let folder_path = PathBuf::from(folder);
    if !folder_path.is_dir() {
        app_logger::error(
            "folder_watch",
            &format!("cannot watch non-dir: {folder}"),
        );
        return None;
    }

    let mut watcher = match notify::recommended_watcher(
        move |res: Result<notify::Event, notify::Error>| match res {
            Ok(event) => {
                if !is_create_or_modify(&event.kind) {
                    return;
                }
                for path in event.paths {
                    if is_xml_path(&path) {
                        let _ = tx.send(path);
                    }
                }
            }
            Err(err) => {
                app_logger::error("folder_watch", &format!("notify error: {err}"));
            }
        },
    ) {
        Ok(w) => w,
        Err(err) => {
            app_logger::error("folder_watch", &format!("create watcher failed: {err}"));
            return None;
        }
    };

    if let Err(err) = watcher.watch(&folder_path, RecursiveMode::NonRecursive) {
        app_logger::error(
            "folder_watch",
            &format!("watch({}) failed: {err}", folder_path.display()),
        );
        return None;
    }

    app_logger::info(
        "folder_watch",
        &format!("fs watcher attached: {}", folder_path.display()),
    );
    Some(watcher)
}

fn is_create_or_modify(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any
    )
}

fn is_xml_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("xml"))
        .unwrap_or(false)
}

async fn insert_new_poll_blocking(app: &AppHandle) -> Result<InsertNewResult, String> {
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        let db = app
            .try_state::<AppDb>()
            .ok_or_else(|| "AppDb chưa sẵn sàng.".to_string())?;
        xml_track::insert_new_xml_files_only(&db, DEVICE_KEY, MIN_FILE_AGE)
    })
    .await
    .map_err(|e| format!("spawn_blocking insert_new: {e}"))?
}

async fn insert_paths_blocking(
    app: &AppHandle,
    paths: Vec<PathBuf>,
) -> Result<InsertNewResult, String> {
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        let db = app
            .try_state::<AppDb>()
            .ok_or_else(|| "AppDb chưa sẵn sàng.".to_string())?;
        let folder = xml_track::get_device_folder(&db, DEVICE_KEY)?
            .tracking_folder
            .unwrap_or_default();

        let mut inserted = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for path in paths {
            let key = path.to_string_lossy().to_string();
            if !seen.insert(key) {
                continue;
            }
            if let Some(row) =
                xml_track::insert_xml_path_if_new(&db, DEVICE_KEY, &path, MIN_FILE_AGE)?
            {
                inserted.push(row);
            }
        }
        Ok(InsertNewResult {
            tracking_folder: folder,
            scanned_count: seen.len(),
            inserted_count: inserted.len(),
            skipped_unstable: 0,
            inserted,
        })
    })
    .await
    .map_err(|e| format!("spawn_blocking insert_paths: {e}"))?
}

async fn auto_process_after_insert(app: &AppHandle, inserted: &[InsertedXmlFile]) {
    if inserted.is_empty() {
        return;
    }
    if !auto_process_enabled(app) {
        app_logger::info(
            "folder_watch",
            "skip auto-process: người dùng tắt tự động xử lý KR-800",
        );
        return;
    }
    if !his_ready(app) {
        app_logger::info(
            "folder_watch",
            "skip auto-process: HIS chưa cấu hình / chưa login",
        );
        emit_auto_process(
            app,
            AutoProcessEvent {
                ok: false,
                message:
                    "Đã index file mới nhưng chưa cấu hình/đăng nhập HIS — bỏ qua tự xử lý."
                        .into(),
                from_time: String::new(),
                to_time: String::new(),
                total: 0,
                processed: 0,
                failed: 0,
                skipped: 0,
                busy: false,
            },
        );
        return;
    }

    let Some((from_time, to_time)) = day_range_for_inserts(inserted) else {
        return;
    };
    run_auto_process(app, &from_time, &to_time).await;
}

/// Xử lý các file waiting còn lại trong ngày hôm nay (local).
async fn auto_process_pending_waiting(app: &AppHandle) {
    if !auto_process_enabled(app) {
        return;
    }
    if !his_ready(app) {
        return;
    }
    let (from_time, to_time) = today_range_local();
    let waiting = {
        let Some(db) = app.try_state::<AppDb>() else {
            return;
        };
        match xml_track::count_waiting_in_range(&db, DEVICE_KEY, &from_time, &to_time) {
            Ok(n) => n,
            Err(_) => return,
        }
    };
    if waiting == 0 {
        return;
    }
    run_auto_process(app, &from_time, &to_time).await;
}

/// Gọi khi user bật toggle auto-process (có folder) — xử lý waiting ngay, không chờ poll.
pub async fn trigger_auto_process_now(app: &AppHandle) {
    auto_process_pending_waiting(app).await;
}

fn auto_process_enabled(app: &AppHandle) -> bool {
    let Some(db) = app.try_state::<AppDb>() else {
        return false;
    };
    xml_track::is_auto_process_enabled(&db, DEVICE_KEY)
}

async fn run_auto_process(app: &AppHandle, from_time: &str, to_time: &str) {
    let Some(db) = app.try_state::<AppDb>() else {
        return;
    };
    let Some(process_state) = app.try_state::<Kr800ProcessState>() else {
        return;
    };

    app_logger::info(
        "folder_watch",
        &format!("auto-process start from={from_time} to={to_time}"),
    );

    match kr800_process::try_process(app, &db, &process_state, from_time, to_time).await {
        Ok(Some(result)) => {
            app_logger::info(
                "folder_watch",
                &format!(
                    "auto-process done total={} processed={} failed={} skipped={}",
                    result.total, result.processed, result.failed, result.skipped
                ),
            );
            emit_auto_process(
                app,
                auto_event_from_result(result, from_time, to_time, false),
            );
        }
        Ok(None) => {
            app_logger::info("folder_watch", "auto-process skipped: pipeline busy");
            emit_auto_process(
                app,
                AutoProcessEvent {
                    ok: true,
                    message: "Pipeline đang bận — sẽ thử lại sau.".into(),
                    from_time: from_time.into(),
                    to_time: to_time.into(),
                    total: 0,
                    processed: 0,
                    failed: 0,
                    skipped: 0,
                    busy: true,
                },
            );
        }
        Err(err) => {
            app_logger::error("folder_watch", &format!("auto-process failed: {err}"));
            emit_auto_process(
                app,
                AutoProcessEvent {
                    ok: false,
                    message: err,
                    from_time: from_time.into(),
                    to_time: to_time.into(),
                    total: 0,
                    processed: 0,
                    failed: 0,
                    skipped: 0,
                    busy: false,
                },
            );
        }
    }
}

fn his_ready(app: &AppHandle) -> bool {
    let Some(db) = app.try_state::<AppDb>() else {
        return false;
    };
    let Ok(settings) = settings::load(&db) else {
        return false;
    };
    !settings.his_api_url.trim().is_empty() && !settings.username.trim().is_empty()
}

/// Khoảng cả ngày (local) bao phủ mọi `created_at` của batch insert.
fn day_range_for_inserts(inserted: &[InsertedXmlFile]) -> Option<(String, String)> {
    let mut min_date: Option<NaiveDate> = None;
    let mut max_date: Option<NaiveDate> = None;
    for item in inserted {
        let Ok(dt) = NaiveDateTime::parse_from_str(&item.created_at, "%Y-%m-%d %H:%M:%S") else {
            continue;
        };
        let d = dt.date();
        min_date = Some(min_date.map(|m| m.min(d)).unwrap_or(d));
        max_date = Some(max_date.map(|m| m.max(d)).unwrap_or(d));
    }
    let min_date = min_date?;
    let max_date = max_date?;
    let from = NaiveDateTime::new(min_date, NaiveTime::from_hms_opt(0, 0, 0)?);
    let to = NaiveDateTime::new(max_date, NaiveTime::from_hms_opt(23, 59, 59)?);
    Some((
        from.format("%Y-%m-%d %H:%M:%S").to_string(),
        to.format("%Y-%m-%d %H:%M:%S").to_string(),
    ))
}

fn today_range_local() -> (String, String) {
    let today = Local::now().date_naive();
    let from = NaiveDateTime::new(
        today,
        NaiveTime::from_hms_opt(0, 0, 0).unwrap_or(NaiveTime::MIN),
    );
    let to = NaiveDateTime::new(
        today,
        NaiveTime::from_hms_opt(23, 59, 59).unwrap_or(NaiveTime::MIN),
    );
    (
        from.format("%Y-%m-%d %H:%M:%S").to_string(),
        to.format("%Y-%m-%d %H:%M:%S").to_string(),
    )
}

fn auto_event_from_result(
    result: ProcessResult,
    from_time: &str,
    to_time: &str,
    busy: bool,
) -> AutoProcessEvent {
    let message = if result.total == 0 {
        "Không có file chờ xử lý trong khoảng tự động.".into()
    } else if result.awaiting_pair > 0
        && result.processed == 0
        && result.failed == 0
        && result.skipped == 0
    {
        // Chỉ nhận lần đo 1 — thông tin bình thường, không phải lỗi HIS.
        format!(
            "Đã nhận {} lần đo 1, đang chờ lần đo 2.",
            result.awaiting_pair
        )
    } else if result.awaiting_pair > 0 {
        format!(
            "Tự xử lý: {} cặp thành công; chờ lần đo 2: {}; bỏ qua {}; lỗi {}.",
            result.processed, result.awaiting_pair, result.skipped, result.failed
        )
    } else {
        format!(
            "Tự xử lý: {}/{} thành công; bỏ qua {}; lỗi {}.",
            result.processed, result.total, result.skipped, result.failed
        )
    };
    AutoProcessEvent {
        ok: true,
        message,
        from_time: from_time.into(),
        to_time: to_time.into(),
        total: result.total,
        processed: result.processed,
        failed: result.failed,
        skipped: result.skipped,
        busy,
    }
}

fn emit_indexed(app: &AppHandle, source: &str, insert: &InsertNewResult) {
    let payload = FilesIndexedEvent {
        source: source.into(),
        inserted_count: insert.inserted_count,
        scanned_count: insert.scanned_count,
        tracking_folder: insert.tracking_folder.clone(),
        inserted: insert.inserted.clone(),
    };
    if let Err(err) = app.emit(EVENT_INDEXED, payload) {
        app_logger::error("folder_watch", &format!("emit {EVENT_INDEXED}: {err}"));
    }
}

fn emit_auto_process(app: &AppHandle, payload: AutoProcessEvent) {
    if let Err(err) = app.emit(EVENT_AUTO_PROCESS, &payload) {
        app_logger::error(
            "folder_watch",
            &format!("emit {EVENT_AUTO_PROCESS}: {err}"),
        );
    }
}

fn emit_watch_status(
    app: &AppHandle,
    active: bool,
    tracking_folder: Option<String>,
    message: &str,
) {
    let payload = WatchStatusEvent {
        active,
        tracking_folder,
        message: message.into(),
    };
    if let Err(err) = app.emit(EVENT_WATCH_STATUS, payload) {
        app_logger::error(
            "folder_watch",
            &format!("emit {EVENT_WATCH_STATUS}: {err}"),
        );
    }
}
