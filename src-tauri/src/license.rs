use crate::license_core::{self, LicenseErrorCode};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

const LICENSE_FILE_NAME: &str = "license.key";

// Development fallback public key generated from a deterministic test seed.
// Replace this at build time with HIS_XML_LICENSE_PUBLIC_KEY for production.
const DEV_PUBLIC_KEY_BASE64URL: &str = "6kpsY-KcUgq-9VB7Ey7F-ZVHdq6-vnuSQh7qaRRG0iw";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseInfo {
    pub customer_name: String,
    pub facility_name: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseStatus {
    pub valid: bool,
    pub info: Option<LicenseInfo>,
}

pub fn current_status() -> LicenseStatus {
    let Some(saved_key) = read_saved_license_key() else {
        return LicenseStatus {
            valid: false,
            info: None,
        };
    };

    match verify_app_license(&saved_key) {
        Ok(info) => LicenseStatus {
            valid: true,
            info: Some(info),
        },
        Err(_) => LicenseStatus {
            valid: false,
            info: None,
        },
    }
}

pub fn activate(key: &str) -> Result<LicenseInfo, String> {
    let info = verify_app_license(key).map_err(|error| error.to_string())?;
    save_license_key(key).map_err(|_| LicenseErrorCode::Unknown.to_string())?;
    Ok(info)
}

fn verify_app_license(key: &str) -> Result<LicenseInfo, LicenseErrorCode> {
    let info = license_core::verify_license(
        key,
        app_public_key(),
        current_machine_id().as_deref(),
        Utc::now(),
    )?;

    Ok(LicenseInfo {
        customer_name: info.customer_name,
        facility_name: info.facility_name,
        expires_at: info.expires_at,
    })
}

fn app_public_key() -> &'static str {
    option_env!("HIS_XML_LICENSE_PUBLIC_KEY").unwrap_or(DEV_PUBLIC_KEY_BASE64URL)
}

fn current_machine_id() -> Option<String> {
    std::env::var("HIS_XML_MACHINE_ID").ok()
}

fn read_saved_license_key() -> Option<String> {
    fs::read_to_string(license_path()).ok()
}

fn save_license_key(key: &str) -> std::io::Result<()> {
    let path = license_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, key.trim())
}

fn license_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("his-xml-sync")
        .join(LICENSE_FILE_NAME)
}
