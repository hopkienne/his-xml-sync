use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// SQLite connection shared across Tauri commands.
pub struct AppDb {
    pub conn: Mutex<Connection>,
    pub path: PathBuf,
}

pub fn init(app: &AppHandle) -> Result<AppDb, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Không lấy được app data dir: {error}"))?;

    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Không tạo được thư mục dữ liệu: {error}"))?;

    let path = dir.join("app.db");
    let conn = Connection::open(&path)
        .map_err(|error| format!("Không mở được SQLite ({}): {error}", path.display()))?;

    migrate(&conn)?;

    Ok(AppDb {
        conn: Mutex::new(conn),
        path,
    })
}

/// Mở DB in-memory đã migrate — dùng cho unit/integration tests.
#[cfg(test)]
pub fn open_memory_for_test() -> Result<AppDb, String> {
    let conn = Connection::open_in_memory()
        .map_err(|error| format!("Không mở được SQLite in-memory: {error}"))?;
    migrate(&conn)?;
    Ok(AppDb {
        conn: Mutex::new(conn),
        path: PathBuf::from(":memory:"),
    })
}

pub(crate) fn migrate(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS app_config (
          id              INTEGER PRIMARY KEY CHECK (id = 1),
          his_api_url     TEXT    NOT NULL DEFAULT '',
          ds_co_so_kcb_id INTEGER NOT NULL DEFAULT 4,
          copy_refraction_to_new_glasses INTEGER NOT NULL DEFAULT 0,
          username        TEXT    NOT NULL DEFAULT '',
          password        TEXT    NOT NULL DEFAULT '',
          created_at      TEXT    NOT NULL,
          updated_at      TEXT    NOT NULL
        );

        INSERT OR IGNORE INTO app_config (
          id, his_api_url, username, password, created_at, updated_at
        ) VALUES (
          1, '', '', '', datetime('now'), datetime('now')
        );

        -- Thư mục tracking theo từng máy (vd. kr-800)
        CREATE TABLE IF NOT EXISTS device_config (
          device_key            TEXT PRIMARY KEY,
          tracking_folder       TEXT NOT NULL DEFAULT '',
          auto_process_enabled  INTEGER NOT NULL DEFAULT 0,
          -- JSON array [{key, value}] query params API danh sách người bệnh
          patient_query_params  TEXT,
          updated_at            TEXT NOT NULL
        );

        -- File XML đã phát hiện trong folder tracking
        -- created_at: thời gian tạo file (parse từ tên file, vd. ..._20260707_145000_...)
        CREATE TABLE IF NOT EXISTS xml_files (
          id                 INTEGER PRIMARY KEY AUTOINCREMENT,
          device_key         TEXT    NOT NULL DEFAULT 'kr-800',
          file_name          TEXT    NOT NULL,
          file_path          TEXT    NOT NULL UNIQUE,
          file_size          INTEGER,
          file_modified_at   TEXT,
          status             TEXT    NOT NULL DEFAULT 'waiting'
                               CHECK (status IN (
                                 'waiting', 'processing', 'parsed', 'patient_matched', 'mapped',
                                 'sending', 'processed', 'patient_not_found', 'treatment_ambiguous',
                                 'xml_error', 'mapping_error', 'send_error', 'failed',
                                 'awaiting_pair', 'pairing', 'pairing_error', 'extra_measurement'
                               )),
          error_message      TEXT,
          content_hash       TEXT,
          patient_code       TEXT,
          patient_no         INTEGER,
          measured_at        TEXT,
          pair_id            INTEGER,
          pair_order         INTEGER,
          -- JSON snapshot đã parse (patient + R/L eyes + hash); nguồn payload PUT.
          measurement_snapshot TEXT,
          nb_dot_dieu_tri_id INTEGER,
          request_payload    TEXT,
          response_payload   TEXT,
          processed_at       TEXT,
          attempt_count      INTEGER NOT NULL DEFAULT 0,
          created_at         TEXT    NOT NULL,
          updated_at         TEXT    NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_xml_files_device_status
          ON xml_files (device_key, status);

        -- Cặp hai lần đo KR-800 (một PUT HIS / cặp).
        CREATE TABLE IF NOT EXISTS measurement_pairs (
          id                 INTEGER PRIMARY KEY AUTOINCREMENT,
          device_key         TEXT    NOT NULL DEFAULT 'kr-800',
          patient_code       TEXT    NOT NULL,
          patient_code_norm  TEXT    NOT NULL,
          file_id_1          INTEGER,
          file_id_2          INTEGER,
          content_hash_1     TEXT,
          content_hash_2     TEXT,
          patient_no_1       INTEGER,
          patient_no_2       INTEGER,
          measured_at_1      TEXT,
          measured_at_2      TEXT,
          status             TEXT    NOT NULL DEFAULT 'awaiting_pair'
                               CHECK (status IN (
                                 'awaiting_pair', 'pairing', 'sending', 'processed',
                                 'pairing_error', 'send_error', 'patient_not_found',
                                 'treatment_ambiguous', 'mapping_error'
                               )),
          nb_dot_dieu_tri_id INTEGER,
          request_payload    TEXT,
          response_payload   TEXT,
          error_message      TEXT,
          attempt_count      INTEGER NOT NULL DEFAULT 0,
          -- Thời điểm claim sending (audit / orphan recovery).
          sending_started_at TEXT,
          processed_at       TEXT,
          created_at         TEXT    NOT NULL,
          updated_at         TEXT    NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_measurement_pairs_patient
          ON measurement_pairs (device_key, patient_code_norm, status);

        -- Session đăng nhập HIS (singleton): lưu access_token cho các API sau
        CREATE TABLE IF NOT EXISTS auth_session (
          id              INTEGER PRIMARY KEY CHECK (id = 1),
          access_token    TEXT    NOT NULL DEFAULT '',
          refresh_token   TEXT,
          token_type      TEXT,
          expires_in      INTEGER,
          expiration      TEXT,
          co_so_kcb_id    INTEGER,
          username        TEXT,
          full_name       TEXT,
          updated_at      TEXT    NOT NULL
        );
        "#,
    )
    .map_err(|error| format!("Migration SQLite thất bại: {error}"))?;

    migrate_app_config_remove_viet_nga_url(conn)?;
    migrate_app_config_add_ds_co_so_kcb_id(conn)?;
    migrate_app_config_add_copy_refraction(conn)?;
    migrate_device_config_add_auto_process(conn)?;
    migrate_device_config_add_patient_query_params(conn)?;
    migrate_xml_files_discovered_to_created(conn)?;
    migrate_xml_files_processing_schema(conn)?;
    migrate_xml_files_pairing_schema(conn)?;
    migrate_measurement_pairs_table(conn)?;
    migrate_xml_files_measurement_snapshot(conn)?;
    migrate_measurement_pairs_sending_started_at(conn)?;

    Ok(())
}

/// DB cũ chưa có cờ tự động xử lý KR-800.
fn migrate_device_config_add_auto_process(conn: &Connection) -> Result<(), String> {
    let columns = table_columns(conn, "device_config")?;
    if columns
        .iter()
        .any(|column| column == "auto_process_enabled")
    {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE device_config ADD COLUMN auto_process_enabled INTEGER NOT NULL DEFAULT 0;",
    )
    .map_err(|error| format!("Thêm auto_process_enabled vào device_config thất bại: {error}"))
}

/// DB cũ chưa có query params API danh sách người bệnh (KR-800).
fn migrate_device_config_add_patient_query_params(conn: &Connection) -> Result<(), String> {
    let columns = table_columns(conn, "device_config")?;
    if columns
        .iter()
        .any(|column| column == "patient_query_params")
    {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE device_config ADD COLUMN patient_query_params TEXT;",
    )
    .map_err(|error| {
        format!("Thêm patient_query_params vào device_config thất bại: {error}")
    })
}

/// DB cũ lưu hai URL trùng mục đích. Rebuild bảng để bỏ `viet_nga_url` và giữ dữ liệu.
fn migrate_app_config_remove_viet_nga_url(conn: &Connection) -> Result<(), String> {
    let columns = table_columns(conn, "app_config")?;
    if !columns.iter().any(|column| column == "viet_nga_url") {
        return Ok(());
    }

    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("Bắt đầu migration app_config thất bại: {error}"))?;

    transaction
        .execute_batch(
            r#"
            DROP TABLE IF EXISTS app_config_migrated;

            CREATE TABLE app_config_migrated (
              id              INTEGER PRIMARY KEY CHECK (id = 1),
              his_api_url     TEXT    NOT NULL DEFAULT '',
              ds_co_so_kcb_id INTEGER NOT NULL DEFAULT 4,
              copy_refraction_to_new_glasses INTEGER NOT NULL DEFAULT 0,
              username        TEXT    NOT NULL DEFAULT '',
              password        TEXT    NOT NULL DEFAULT '',
              created_at      TEXT    NOT NULL,
              updated_at      TEXT    NOT NULL
            );

            INSERT INTO app_config_migrated (
              id, his_api_url, ds_co_so_kcb_id, copy_refraction_to_new_glasses,
              username, password, created_at, updated_at
            )
            SELECT
              id,
              CASE
                WHEN trim(his_api_url) <> '' THEN his_api_url
                ELSE viet_nga_url
              END,
              4,
              0,
              username,
              password,
              created_at,
              updated_at
            FROM app_config;

            DROP TABLE app_config;
            ALTER TABLE app_config_migrated RENAME TO app_config;
            "#,
        )
        .map_err(|error| format!("Rebuild app_config thất bại: {error}"))?;

    transaction
        .commit()
        .map_err(|error| format!("Commit migration app_config thất bại: {error}"))
}

/// Bổ sung ID cơ sở khám bệnh cho DB đã tồn tại trước phiên bản cấu hình này.
fn migrate_app_config_add_ds_co_so_kcb_id(conn: &Connection) -> Result<(), String> {
    let columns = table_columns(conn, "app_config")?;
    if columns.iter().any(|column| column == "ds_co_so_kcb_id") {
        return Ok(());
    }

    conn.execute_batch(
        "ALTER TABLE app_config ADD COLUMN ds_co_so_kcb_id INTEGER NOT NULL DEFAULT 4;",
    )
    .map_err(|error| format!("Thêm ds_co_so_kcb_id vào app_config thất bại: {error}"))
}

fn migrate_app_config_add_copy_refraction(conn: &Connection) -> Result<(), String> {
    let columns = table_columns(conn, "app_config")?;
    if columns
        .iter()
        .any(|column| column == "copy_refraction_to_new_glasses")
    {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE app_config ADD COLUMN copy_refraction_to_new_glasses INTEGER NOT NULL DEFAULT 0;",
    )
    .map_err(|error| format!("Thêm cấu hình copy refraction thất bại: {error}"))
}

/// DB cũ dùng `discovered_at` (thời điểm quét) → đổi thành `created_at` (thời gian từ tên file).
fn migrate_xml_files_discovered_to_created(conn: &Connection) -> Result<(), String> {
    let columns = table_columns(conn, "xml_files")?;
    let has_discovered = columns.iter().any(|c| c == "discovered_at");
    let has_created = columns.iter().any(|c| c == "created_at");

    if has_discovered && !has_created {
        conn.execute_batch("ALTER TABLE xml_files RENAME COLUMN discovered_at TO created_at")
            .map_err(|e| format!("Rename discovered_at → created_at thất bại: {e}"))?;
    }

    // Index filter theo created_at (CREATE IF NOT EXISTS an toàn khi chạy lại).
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_xml_files_device_created ON xml_files (device_key, created_at);",
    )
    .map_err(|e| format!("Tạo index created_at thất bại: {e}"))?;

    Ok(())
}

fn migrate_xml_files_processing_schema(conn: &Connection) -> Result<(), String> {
    let columns = table_columns(conn, "xml_files")?;
    let required = [
        "content_hash",
        "patient_code",
        "nb_dot_dieu_tri_id",
        "request_payload",
        "response_payload",
        "processed_at",
        "attempt_count",
    ];
    let has_columns = required
        .iter()
        .all(|required| columns.iter().any(|column| column == required));
    let schema: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'xml_files'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("Đọc schema xml_files thất bại: {error}"))?;
    // Pairing migration sẽ rebuild tiếp nếu thiếu awaiting_pair.
    if has_columns && schema.contains("patient_not_found") {
        return Ok(());
    }

    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("Bắt đầu migration xml_files thất bại: {error}"))?;
    transaction
        .execute_batch(
            r#"
            DROP TABLE IF EXISTS xml_files_migrated;
            CREATE TABLE xml_files_migrated (
              id                 INTEGER PRIMARY KEY AUTOINCREMENT,
              device_key         TEXT    NOT NULL DEFAULT 'kr-800',
              file_name          TEXT    NOT NULL,
              file_path          TEXT    NOT NULL UNIQUE,
              file_size          INTEGER,
              file_modified_at   TEXT,
              status             TEXT    NOT NULL DEFAULT 'waiting'
                                   CHECK (status IN (
                                     'waiting', 'processing', 'parsed', 'patient_matched', 'mapped',
                                     'sending', 'processed', 'patient_not_found', 'treatment_ambiguous',
                                     'xml_error', 'mapping_error', 'send_error', 'failed',
                                     'awaiting_pair', 'pairing', 'pairing_error', 'extra_measurement'
                                   )),
              error_message      TEXT,
              content_hash       TEXT,
              patient_code       TEXT,
              patient_no         INTEGER,
              measured_at        TEXT,
              pair_id            INTEGER,
              pair_order         INTEGER,
              nb_dot_dieu_tri_id INTEGER,
              request_payload    TEXT,
              response_payload   TEXT,
              processed_at       TEXT,
              attempt_count      INTEGER NOT NULL DEFAULT 0,
              created_at         TEXT    NOT NULL,
              updated_at         TEXT    NOT NULL
            );
            INSERT INTO xml_files_migrated (
              id, device_key, file_name, file_path, file_size, file_modified_at,
              status, error_message, created_at, updated_at
            )
            SELECT
              id, device_key, file_name, file_path, file_size, file_modified_at,
              status, error_message, created_at, updated_at
            FROM xml_files;
            DROP TABLE xml_files;
            ALTER TABLE xml_files_migrated RENAME TO xml_files;
            CREATE INDEX IF NOT EXISTS idx_xml_files_device_status
              ON xml_files (device_key, status);
            CREATE INDEX IF NOT EXISTS idx_xml_files_device_created
              ON xml_files (device_key, created_at);
            CREATE INDEX IF NOT EXISTS idx_xml_files_content_hash
              ON xml_files (content_hash, status);
            "#,
        )
        .map_err(|error| format!("Rebuild xml_files thất bại: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("Commit migration xml_files thất bại: {error}"))
}

/// Mở rộng xml_files cho workflow ghép hai lần đo KR-800 (không mất audit).
fn migrate_xml_files_pairing_schema(conn: &Connection) -> Result<(), String> {
    let columns = table_columns(conn, "xml_files")?;
    let schema: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'xml_files'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("Đọc schema xml_files thất bại: {error}"))?;

    let has_pair_cols = ["patient_no", "measured_at", "pair_id", "pair_order"]
        .iter()
        .all(|name| columns.iter().any(|column| column == name));
    if has_pair_cols && schema.contains("awaiting_pair") {
        return Ok(());
    }

    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("Bắt đầu migration pairing xml_files thất bại: {error}"))?;
    transaction
        .execute_batch(
            r#"
            DROP TABLE IF EXISTS xml_files_pairing_migrated;
            CREATE TABLE xml_files_pairing_migrated (
              id                 INTEGER PRIMARY KEY AUTOINCREMENT,
              device_key         TEXT    NOT NULL DEFAULT 'kr-800',
              file_name          TEXT    NOT NULL,
              file_path          TEXT    NOT NULL UNIQUE,
              file_size          INTEGER,
              file_modified_at   TEXT,
              status             TEXT    NOT NULL DEFAULT 'waiting'
                                   CHECK (status IN (
                                     'waiting', 'processing', 'parsed', 'patient_matched', 'mapped',
                                     'sending', 'processed', 'patient_not_found', 'treatment_ambiguous',
                                     'xml_error', 'mapping_error', 'send_error', 'failed',
                                     'awaiting_pair', 'pairing', 'pairing_error', 'extra_measurement'
                                   )),
              error_message      TEXT,
              content_hash       TEXT,
              patient_code       TEXT,
              patient_no         INTEGER,
              measured_at        TEXT,
              pair_id            INTEGER,
              pair_order         INTEGER,
              nb_dot_dieu_tri_id INTEGER,
              request_payload    TEXT,
              response_payload   TEXT,
              processed_at       TEXT,
              attempt_count      INTEGER NOT NULL DEFAULT 0,
              created_at         TEXT    NOT NULL,
              updated_at         TEXT    NOT NULL
            );

            INSERT INTO xml_files_pairing_migrated (
              id, device_key, file_name, file_path, file_size, file_modified_at,
              status, error_message, content_hash, patient_code, patient_no, measured_at,
              pair_id, pair_order, nb_dot_dieu_tri_id, request_payload, response_payload,
              processed_at, attempt_count, created_at, updated_at
            )
            SELECT
              id, device_key, file_name, file_path, file_size, file_modified_at,
              status, error_message, content_hash, patient_code,
              NULL, NULL, NULL, NULL,
              nb_dot_dieu_tri_id, request_payload, response_payload,
              processed_at, attempt_count, created_at, updated_at
            FROM xml_files;

            DROP TABLE xml_files;
            ALTER TABLE xml_files_pairing_migrated RENAME TO xml_files;
            CREATE INDEX IF NOT EXISTS idx_xml_files_device_status
              ON xml_files (device_key, status);
            CREATE INDEX IF NOT EXISTS idx_xml_files_device_created
              ON xml_files (device_key, created_at);
            CREATE INDEX IF NOT EXISTS idx_xml_files_content_hash
              ON xml_files (content_hash, status);
            CREATE INDEX IF NOT EXISTS idx_xml_files_patient_code
              ON xml_files (device_key, patient_code);
            CREATE INDEX IF NOT EXISTS idx_xml_files_pair_id
              ON xml_files (pair_id);
            "#,
        )
        .map_err(|error| format!("Rebuild xml_files pairing thất bại: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("Commit migration pairing xml_files thất bại: {error}"))
}

fn migrate_measurement_pairs_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS measurement_pairs (
          id                 INTEGER PRIMARY KEY AUTOINCREMENT,
          device_key         TEXT    NOT NULL DEFAULT 'kr-800',
          patient_code       TEXT    NOT NULL,
          patient_code_norm  TEXT    NOT NULL,
          file_id_1          INTEGER,
          file_id_2          INTEGER,
          content_hash_1     TEXT,
          content_hash_2     TEXT,
          patient_no_1       INTEGER,
          patient_no_2       INTEGER,
          measured_at_1      TEXT,
          measured_at_2      TEXT,
          status             TEXT    NOT NULL DEFAULT 'awaiting_pair'
                               CHECK (status IN (
                                 'awaiting_pair', 'pairing', 'sending', 'processed',
                                 'pairing_error', 'send_error', 'patient_not_found',
                                 'treatment_ambiguous', 'mapping_error'
                               )),
          nb_dot_dieu_tri_id INTEGER,
          request_payload    TEXT,
          response_payload   TEXT,
          error_message      TEXT,
          attempt_count      INTEGER NOT NULL DEFAULT 0,
          sending_started_at TEXT,
          processed_at       TEXT,
          created_at         TEXT    NOT NULL,
          updated_at         TEXT    NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_measurement_pairs_patient
          ON measurement_pairs (device_key, patient_code_norm, status);
        "#,
    )
    .map_err(|error| format!("Tạo measurement_pairs thất bại: {error}"))
}

/// Snapshot parse (eyes + meta) — không rebuild payload từ file mutable.
fn migrate_xml_files_measurement_snapshot(conn: &Connection) -> Result<(), String> {
    let columns = table_columns(conn, "xml_files")?;
    if columns.iter().any(|column| column == "measurement_snapshot") {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE xml_files ADD COLUMN measurement_snapshot TEXT;")
        .map_err(|error| format!("Thêm measurement_snapshot thất bại: {error}"))
}

/// Mốc claim sending để audit / orphan recovery sau crash.
fn migrate_measurement_pairs_sending_started_at(conn: &Connection) -> Result<(), String> {
    let columns = table_columns(conn, "measurement_pairs")?;
    if columns.iter().any(|column| column == "sending_started_at") {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE measurement_pairs ADD COLUMN sending_started_at TEXT;")
        .map_err(|error| format!("Thêm sending_started_at thất bại: {error}"))
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| format!("PRAGMA table_info({table}) thất bại: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("Đọc cột {table} thất bại: {e}"))?;
    let mut cols = Vec::new();
    for row in rows {
        cols.push(row.map_err(|e| format!("Map cột {table} thất bại: {e}"))?);
    }
    Ok(cols)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_app_config_to_one_api_url() {
        let conn = Connection::open_in_memory().expect("open in-memory SQLite");
        conn.execute_batch(
            r#"
            CREATE TABLE app_config (
              id             INTEGER PRIMARY KEY CHECK (id = 1),
              his_api_url    TEXT NOT NULL DEFAULT '',
              viet_nga_url   TEXT NOT NULL DEFAULT '',
              username       TEXT NOT NULL DEFAULT '',
              password       TEXT NOT NULL DEFAULT '',
              created_at     TEXT NOT NULL,
              updated_at     TEXT NOT NULL
            );
            INSERT INTO app_config (
              id, his_api_url, viet_nga_url, username, password, created_at, updated_at
            ) VALUES (
              1, '', 'https://legacy.example', 'doctor', 'secret',
              '2026-01-01 00:00:00', '2026-02-01 00:00:00'
            );
            "#,
        )
        .expect("create legacy app_config");

        migrate(&conn).expect("migrate legacy database");

        assert_eq!(
            table_columns(&conn, "app_config").expect("read app_config columns"),
            vec![
                "id",
                "his_api_url",
                "ds_co_so_kcb_id",
                "copy_refraction_to_new_glasses",
                "username",
                "password",
                "created_at",
                "updated_at"
            ]
        );

        let saved = conn
            .query_row(
                "SELECT his_api_url, ds_co_so_kcb_id, copy_refraction_to_new_glasses, username, password, created_at, updated_at FROM app_config WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, bool>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .expect("read migrated app_config");

        assert_eq!(saved.0, "https://legacy.example");
        assert_eq!(saved.1, 4);
        assert!(!saved.2);
        assert_eq!(saved.3, "doctor");
        assert_eq!(saved.4, "secret");
        assert_eq!(saved.5, "2026-01-01 00:00:00");
        assert_eq!(saved.6, "2026-02-01 00:00:00");
    }

    #[test]
    fn adds_default_facility_id_to_existing_app_config() {
        let conn = Connection::open_in_memory().expect("open in-memory SQLite");
        conn.execute_batch(
            r#"
            CREATE TABLE app_config (
              id           INTEGER PRIMARY KEY CHECK (id = 1),
              his_api_url  TEXT NOT NULL DEFAULT '',
              username     TEXT NOT NULL DEFAULT '',
              password     TEXT NOT NULL DEFAULT '',
              created_at   TEXT NOT NULL,
              updated_at   TEXT NOT NULL
            );
            INSERT INTO app_config (
              id, his_api_url, username, password, created_at, updated_at
            ) VALUES (
              1, 'https://his.example', 'doctor', 'secret',
              '2026-01-01 00:00:00', '2026-02-01 00:00:00'
            );
            "#,
        )
        .expect("create existing app_config");

        migrate(&conn).expect("migrate existing database");

        let facility_id = conn
            .query_row(
                "SELECT ds_co_so_kcb_id FROM app_config WHERE id = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("read default facility id");

        assert_eq!(facility_id, 4);
    }

    #[test]
    fn migrates_xml_files_to_processing_audit_schema() {
        let conn = Connection::open_in_memory().expect("open in-memory SQLite");
        conn.execute_batch(
            r#"
            CREATE TABLE xml_files (
              id               INTEGER PRIMARY KEY AUTOINCREMENT,
              device_key       TEXT NOT NULL DEFAULT 'kr-800',
              file_name        TEXT NOT NULL,
              file_path        TEXT NOT NULL UNIQUE,
              file_size        INTEGER,
              file_modified_at TEXT,
              status           TEXT NOT NULL DEFAULT 'waiting'
                                 CHECK (status IN ('waiting', 'processed', 'failed')),
              error_message    TEXT,
              created_at       TEXT NOT NULL,
              updated_at       TEXT NOT NULL
            );
            INSERT INTO xml_files (
              device_key, file_name, file_path, status, created_at, updated_at
            ) VALUES (
              'kr-800', 'sample.xml', '/tmp/sample.xml', 'processed',
              '2026-07-07 14:50:00', '2026-07-07 14:51:00'
            );
            "#,
        )
        .expect("create legacy xml_files");

        migrate(&conn).expect("migrate xml_files");

        let columns = table_columns(&conn, "xml_files").expect("read xml columns");
        for expected in [
            "content_hash",
            "patient_code",
            "nb_dot_dieu_tri_id",
            "request_payload",
            "response_payload",
            "processed_at",
            "attempt_count",
        ] {
            assert!(columns.iter().any(|column| column == expected));
        }
        let status: String = conn
            .query_row("SELECT status FROM xml_files WHERE id = 1", [], |row| {
                row.get(0)
            })
            .expect("preserve old row");
        assert_eq!(status, "processed");
        conn.execute(
            "UPDATE xml_files SET status = 'mapping_error' WHERE id = 1",
            [],
        )
        .expect("new detailed status is accepted");
    }

    #[test]
    fn migrates_pairing_schema_preserves_processed_audit() {
        let conn = Connection::open_in_memory().expect("open in-memory SQLite");
        conn.execute_batch(
            r#"
            CREATE TABLE xml_files (
              id                 INTEGER PRIMARY KEY AUTOINCREMENT,
              device_key         TEXT NOT NULL DEFAULT 'kr-800',
              file_name          TEXT NOT NULL,
              file_path          TEXT NOT NULL UNIQUE,
              file_size          INTEGER,
              file_modified_at   TEXT,
              status             TEXT NOT NULL DEFAULT 'waiting'
                                 CHECK (status IN (
                                   'waiting', 'processing', 'parsed', 'patient_matched', 'mapped',
                                   'sending', 'processed', 'patient_not_found', 'treatment_ambiguous',
                                   'xml_error', 'mapping_error', 'send_error', 'failed'
                                 )),
              error_message      TEXT,
              content_hash       TEXT,
              patient_code       TEXT,
              nb_dot_dieu_tri_id INTEGER,
              request_payload    TEXT,
              response_payload   TEXT,
              processed_at       TEXT,
              attempt_count      INTEGER NOT NULL DEFAULT 0,
              created_at         TEXT NOT NULL,
              updated_at         TEXT NOT NULL
            );
            INSERT INTO xml_files (
              device_key, file_name, file_path, status, content_hash, patient_code,
              nb_dot_dieu_tri_id, request_payload, response_payload, processed_at,
              attempt_count, created_at, updated_at
            ) VALUES (
              'kr-800', 'old.xml', '/tmp/old.xml', 'processed',
              'abc123hash', 'HCM2607150275', 99,
              '{"legacy":true}', '{"ok":true}', '2026-07-15 16:00:00',
              2, '2026-07-15 15:12:40', '2026-07-15 16:00:00'
            );
            "#,
        )
        .expect("create pre-pairing xml_files");

        migrate(&conn).expect("migrate to pairing schema");

        let columns = table_columns(&conn, "xml_files").expect("columns");
        for expected in [
            "content_hash",
            "patient_code",
            "nb_dot_dieu_tri_id",
            "request_payload",
            "response_payload",
            "processed_at",
            "attempt_count",
            "patient_no",
            "measured_at",
            "pair_id",
            "pair_order",
            "measurement_snapshot",
        ] {
            assert!(
                columns.iter().any(|column| column == expected),
                "missing column {expected}"
            );
        }

        let pair_cols = table_columns(&conn, "measurement_pairs").expect("pair columns");
        assert!(
            pair_cols.iter().any(|c| c == "sending_started_at"),
            "missing sending_started_at"
        );

        let row = conn
            .query_row(
                r#"
                SELECT status, content_hash, patient_code, nb_dot_dieu_tri_id,
                       request_payload, response_payload, processed_at, attempt_count
                FROM xml_files WHERE file_path = '/tmp/old.xml'
                "#,
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .expect("read preserved audit");

        assert_eq!(row.0, "processed");
        assert_eq!(row.1, "abc123hash");
        assert_eq!(row.2, "HCM2607150275");
        assert_eq!(row.3, 99);
        assert_eq!(row.4, r#"{"legacy":true}"#);
        assert_eq!(row.5, r#"{"ok":true}"#);
        assert_eq!(row.6, "2026-07-15 16:00:00");
        assert_eq!(row.7, 2);

        // Không tự chuyển processed → waiting (không gửi lại sau migration).
        let waiting: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM xml_files WHERE status = 'waiting'",
                [],
                |row| row.get(0),
            )
            .expect("count waiting");
        assert_eq!(waiting, 0);

        conn.execute(
            "UPDATE xml_files SET status = 'awaiting_pair' WHERE id = 1",
            [],
        )
        .expect("new pairing status accepted");

        let pairs_exist: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='measurement_pairs'",
                [],
                |row| row.get(0),
            )
            .expect("pairs table");
        assert_eq!(pairs_exist, 1);
    }
}
