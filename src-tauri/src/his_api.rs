use crate::app_logger;
use crate::db::AppDb;
use crate::settings::{self, AppSettings};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::MutexGuard;
use std::time::Duration;

const LOGIN_PATH: &str = "/api/his/v1/auth/login";
const HTTP_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HisAuthStatus {
    pub logged_in: bool,
    pub username: Option<String>,
    pub full_name: Option<String>,
    pub co_so_kcb_id: Option<i64>,
    pub token_type: Option<String>,
    pub expires_in: Option<i64>,
    pub expiration: Option<String>,
    pub updated_at: Option<String>,
    /// true if access_token is present in store (never return the token itself to UI).
    pub has_access_token: bool,
}

#[derive(Debug, Clone)]
pub struct HisSession {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub expires_in: Option<i64>,
    pub expiration: Option<String>,
    pub co_so_kcb_id: Option<i64>,
    pub username: Option<String>,
    pub full_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct LoginRequestBody {
    #[serde(rename = "taiKhoan")]
    tai_khoan: String,
    #[serde(rename = "matKhau")]
    mat_khau: String,
}

#[derive(Debug, Deserialize)]
struct LoginApiEnvelope {
    code: i64,
    message: Option<String>,
    data: Option<LoginData>,
}

#[derive(Debug, Deserialize)]
struct LoginData {
    #[serde(alias = "accessToken")]
    access_token: String,
    #[serde(alias = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(alias = "tokenType")]
    token_type: Option<String>,
    #[serde(alias = "expiresIn")]
    expires_in: Option<i64>,
    expiration: Option<String>,
    #[serde(rename = "coSoKcbId", alias = "co_so_kcb_id")]
    co_so_kcb_id: Option<i64>,
    username: Option<String>,
    #[serde(alias = "fullName")]
    full_name: Option<String>,
}

/// POST login using credentials from `app_config`, then persist tokens.
/// `matKhau` is the MD5 hash already stored in `app_config.password` (hashed on save).
pub async fn login_and_store(db: &AppDb) -> Result<HisAuthStatus, String> {
    let settings = settings::load_with_password(db)?;

    if settings.his_api_url.trim().is_empty() {
        return Err("Chưa cấu hình API URL HIS. Vào Cấu hình để lưu trước.".into());
    }
    if settings.username.trim().is_empty() || settings.password.is_empty() {
        return Err("Chưa cấu hình tài khoản/mật khẩu HIS. Vào Cấu hình để lưu trước.".into());
    }

    let session = login_with_settings(&settings).await?;
    save_session(db, &session)?;
    get_auth_status(db)
}

pub async fn login_with_settings(settings: &AppSettings) -> Result<HisSession, String> {
    let url = join_url(&settings.his_api_url, LOGIN_PATH);
    // Password in settings is already MD5-hashed when saved from Cấu hình.
    let body = LoginRequestBody {
        tai_khoan: settings.username.trim().to_string(),
        mat_khau: settings.password.clone(),
    };

    app_logger::info(
        "his_api",
        &format!(
            "login request POST {url} taiKhoan={} (password redacted)",
            body.tai_khoan
        ),
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("Không tạo được HTTP client: {e}"))?;

    let response = client.post(&url).json(&body).send().await.map_err(|e| {
        let msg = format!("HTTP login thất bại: {e}");
        app_logger::error("his_api", &msg);
        msg
    })?;

    let status = response.status();
    let raw = response
        .text()
        .await
        .map_err(|e| format!("Không đọc được body login: {e}"))?;

    app_logger::info(
        "his_api",
        &format!(
            "login response status={} body_len={}",
            status.as_u16(),
            raw.len()
        ),
    );

    if !status.is_success() {
        let msg = format!(
            "Login HTTP {}: {}",
            status.as_u16(),
            preview_body(&raw, 300)
        );
        app_logger::error("his_api", &msg);
        return Err(msg);
    }

    let envelope: LoginApiEnvelope = serde_json::from_str(&raw).map_err(|e| {
        let msg = format!(
            "Parse login JSON thất bại: {e}; body={}",
            preview_body(&raw, 300)
        );
        app_logger::error("his_api", &msg);
        msg
    })?;

    if envelope.code != 0 {
        let msg = format!(
            "Login API code={}: {}",
            envelope.code,
            envelope.message.unwrap_or_else(|| "unknown".into())
        );
        app_logger::error("his_api", &msg);
        return Err(msg);
    }

    let data = envelope
        .data
        .ok_or_else(|| "Login response thiếu data.".to_string())?;

    if data.access_token.trim().is_empty() {
        return Err("Login response thiếu access_token.".into());
    }

    app_logger::info(
        "his_api",
        &format!(
            "login ok username={:?} full_name={:?} coSoKcbId={:?} expires_in={:?}",
            data.username, data.full_name, data.co_so_kcb_id, data.expires_in
        ),
    );

    Ok(HisSession {
        access_token: data.access_token,
        refresh_token: data.refresh_token,
        token_type: data.token_type,
        expires_in: data.expires_in,
        expiration: data.expiration,
        co_so_kcb_id: data.co_so_kcb_id,
        username: data.username,
        full_name: data.full_name,
    })
}

pub fn save_session(db: &AppDb, session: &HisSession) -> Result<(), String> {
    let conn = lock_conn(db)?;
    conn.execute(
        r#"
        INSERT INTO auth_session (
          id, access_token, refresh_token, token_type, expires_in, expiration,
          co_so_kcb_id, username, full_name, updated_at
        ) VALUES (
          1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now')
        )
        ON CONFLICT(id) DO UPDATE SET
          access_token  = excluded.access_token,
          refresh_token = excluded.refresh_token,
          token_type    = excluded.token_type,
          expires_in    = excluded.expires_in,
          expiration    = excluded.expiration,
          co_so_kcb_id  = excluded.co_so_kcb_id,
          username      = excluded.username,
          full_name     = excluded.full_name,
          updated_at    = datetime('now')
        "#,
        params![
            session.access_token,
            session.refresh_token,
            session.token_type,
            session.expires_in,
            session.expiration,
            session.co_so_kcb_id,
            session.username,
            session.full_name,
        ],
    )
    .map_err(|e| format!("Lưu auth_session thất bại: {e}"))?;

    app_logger::info("his_api", "auth_session saved to SQLite");
    Ok(())
}

pub fn get_auth_status(db: &AppDb) -> Result<HisAuthStatus, String> {
    let conn = lock_conn(db)?;
    let row = conn
        .query_row(
            r#"
            SELECT access_token, token_type, expires_in, expiration,
                   co_so_kcb_id, username, full_name, updated_at
            FROM auth_session
            WHERE id = 1
            "#,
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("Đọc auth_session thất bại: {e}"))?;

    match row {
        Some((
            token,
            token_type,
            expires_in,
            expiration,
            co_so,
            username,
            full_name,
            updated_at,
        )) => Ok(HisAuthStatus {
            logged_in: !token.is_empty(),
            username,
            full_name,
            co_so_kcb_id: co_so,
            token_type,
            expires_in,
            expiration,
            updated_at: Some(updated_at),
            has_access_token: !token.is_empty(),
        }),
        None => Ok(HisAuthStatus {
            logged_in: false,
            username: None,
            full_name: None,
            co_so_kcb_id: None,
            token_type: None,
            expires_in: None,
            expiration: None,
            updated_at: None,
            has_access_token: false,
        }),
    }
}

/// Load access token for subsequent API calls (backend only).
pub fn get_access_token(db: &AppDb) -> Result<Option<String>, String> {
    let conn = lock_conn(db)?;
    let token = conn
        .query_row(
            "SELECT access_token FROM auth_session WHERE id = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Đọc access_token thất bại: {e}"))?;

    Ok(token.filter(|t| !t.is_empty()))
}

pub fn join_url(base: &str, path: &str) -> String {
    let raw_base = base.trim().trim_end_matches('/');
    // If user pasted full login URL, use as-is.
    if raw_base.contains("/auth/login") && path == LOGIN_PATH {
        return raw_base.to_string();
    }
    let base = raw_base
        .split("/api/his/v1/auth/login")
        .next()
        .unwrap_or(raw_base)
        .trim_end_matches('/');
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!("{base}{path}")
}

fn preview_body(raw: &str, max: usize) -> String {
    let collapsed: String = raw
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let collapsed = collapsed.trim();
    if collapsed.chars().count() <= max {
        return collapsed.to_string();
    }
    let truncated: String = collapsed.chars().take(max).collect();
    format!("{truncated}…")
}

fn lock_conn(db: &AppDb) -> Result<MutexGuard<'_, Connection>, String> {
    db.conn
        .lock()
        .map_err(|_| "Không khóa được kết nối SQLite.".to_string())
}
