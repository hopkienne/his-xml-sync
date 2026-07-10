use crate::db::AppDb;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::MutexGuard;

/// Connection config persisted in SQLite `app_config` (singleton id = 1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub his_api_url: String,
    #[serde(default = "default_ds_co_so_kcb_id")]
    pub ds_co_so_kcb_id: i64,
    #[serde(default)]
    pub copy_refraction_to_new_glasses: bool,
    pub username: String,
    /// Write: empty string means "keep existing password" on save.
    /// Read: returns stored password for local desktop form binding.
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            his_api_url: String::new(),
            ds_co_so_kcb_id: default_ds_co_so_kcb_id(),
            copy_refraction_to_new_glasses: false,
            username: String::new(),
            password: String::new(),
            updated_at: None,
        }
    }
}

fn default_ds_co_so_kcb_id() -> i64 {
    4
}

pub fn load(db: &AppDb) -> Result<AppSettings, String> {
    let conn = lock_conn(db)?;
    load_from_conn(&conn)
}

pub fn save(db: &AppDb, incoming: AppSettings) -> Result<AppSettings, String> {
    let conn = lock_conn(db)?;
    let existing = load_from_conn(&conn)?;

    let his_api_url = incoming.his_api_url.trim().to_string();
    let ds_co_so_kcb_id = incoming.ds_co_so_kcb_id;
    let copy_refraction_to_new_glasses = incoming.copy_refraction_to_new_glasses;
    let username = incoming.username.trim().to_string();
    let password = if incoming.password.is_empty() {
        existing.password
    } else {
        incoming.password
    };

    validate(&his_api_url, &username, &password)?;

    conn.execute(
        r#"
        INSERT INTO app_config (
          id, his_api_url, ds_co_so_kcb_id, copy_refraction_to_new_glasses,
          username, password, created_at, updated_at
        ) VALUES (
          1, ?1, ?2, ?3, ?4, ?5, datetime('now'), datetime('now')
        )
        ON CONFLICT(id) DO UPDATE SET
          his_api_url     = excluded.his_api_url,
          ds_co_so_kcb_id = excluded.ds_co_so_kcb_id,
          copy_refraction_to_new_glasses = excluded.copy_refraction_to_new_glasses,
          username = excluded.username,
          password = excluded.password,
          updated_at = datetime('now')
        "#,
        rusqlite::params![
            his_api_url,
            ds_co_so_kcb_id,
            copy_refraction_to_new_glasses,
            username,
            password
        ],
    )
    .map_err(|error| format!("Lưu app_config thất bại: {error}"))?;

    load_from_conn(&conn)
}

fn load_from_conn(conn: &Connection) -> Result<AppSettings, String> {
    conn.query_row(
        r#"
        SELECT his_api_url, ds_co_so_kcb_id, copy_refraction_to_new_glasses,
               username, password, updated_at
        FROM app_config
        WHERE id = 1
        "#,
        [],
        |row| {
            Ok(AppSettings {
                his_api_url: row.get(0)?,
                ds_co_so_kcb_id: row.get(1)?,
                copy_refraction_to_new_glasses: row.get(2)?,
                username: row.get(3)?,
                password: row.get(4)?,
                updated_at: row.get(5)?,
            })
        },
    )
    .map_err(|error| format!("Đọc app_config thất bại: {error}"))
}

fn validate(
    his_api_url: &str,
    username: &str,
    password: &str,
) -> Result<(), String> {
    if his_api_url.is_empty() {
        return Err("Vui lòng nhập API URL HIS.".into());
    }
    if username.is_empty() {
        return Err("Vui lòng nhập tài khoản.".into());
    }
    if password.is_empty() {
        return Err("Vui lòng nhập mật khẩu.".into());
    }
    Ok(())
}

fn lock_conn(db: &AppDb) -> Result<MutexGuard<'_, Connection>, String> {
    db.conn
        .lock()
        .map_err(|_| "Không khóa được kết nối SQLite.".to_string())
}
