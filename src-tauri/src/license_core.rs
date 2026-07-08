use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::{convert::TryFrom, fmt};

const LICENSE_PREFIX: &str = "HXS1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LicensePayload {
    pub version: u8,
    pub license_id: String,
    pub customer_name: String,
    pub facility_name: String,
    pub machine_id: Option<String>,
    pub issued_at: String,
    pub expires_at: String,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LicenseInfo {
    pub customer_name: String,
    pub facility_name: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenseErrorCode {
    InvalidFormat,
    InvalidSignature,
    Expired,
    MachineMismatch,
    Unknown,
}

impl LicenseErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidFormat => "INVALID_FORMAT",
            Self::InvalidSignature => "INVALID_SIGNATURE",
            Self::Expired => "EXPIRED",
            Self::MachineMismatch => "MACHINE_MISMATCH",
            Self::Unknown => "UNKNOWN",
        }
    }
}

impl fmt::Display for LicenseErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub fn sign_license(
    payload: &LicensePayload,
    private_key_base64url: &str,
) -> Result<String, LicenseErrorCode> {
    let signing_key_bytes = decode_fixed_32(private_key_base64url)?;
    let signing_key = SigningKey::from_bytes(&signing_key_bytes);
    let payload_json = serde_json::to_vec(payload).map_err(|_| LicenseErrorCode::InvalidFormat)?;
    let signature = signing_key.sign(&payload_json);

    Ok(format!(
        "{LICENSE_PREFIX}.{}.{}",
        URL_SAFE_NO_PAD.encode(payload_json),
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    ))
}

pub fn verify_license(
    license_key: &str,
    public_key_base64url: &str,
    current_machine_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<LicenseInfo, LicenseErrorCode> {
    let parts: Vec<&str> = license_key.trim().split('.').collect();
    if parts.len() != 3 || parts[0] != LICENSE_PREFIX {
        return Err(LicenseErrorCode::InvalidFormat);
    }

    let payload_bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| LicenseErrorCode::InvalidFormat)?;
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(parts[2])
        .map_err(|_| LicenseErrorCode::InvalidFormat)?;
    let signature = Signature::try_from(signature_bytes.as_slice())
        .map_err(|_| LicenseErrorCode::InvalidFormat)?;

    let verifying_key_bytes = decode_fixed_32(public_key_base64url)?;
    let verifying_key = VerifyingKey::from_bytes(&verifying_key_bytes)
        .map_err(|_| LicenseErrorCode::InvalidFormat)?;

    verifying_key
        .verify(&payload_bytes, &signature)
        .map_err(|_| LicenseErrorCode::InvalidSignature)?;

    let payload: LicensePayload =
        serde_json::from_slice(&payload_bytes).map_err(|_| LicenseErrorCode::InvalidFormat)?;

    if payload.version != 1 {
        return Err(LicenseErrorCode::InvalidFormat);
    }

    let expires_at = DateTime::parse_from_rfc3339(&payload.expires_at)
        .map_err(|_| LicenseErrorCode::InvalidFormat)?
        .with_timezone(&Utc);

    if now > expires_at {
        return Err(LicenseErrorCode::Expired);
    }

    if let Some(licensed_machine_id) = payload.machine_id.as_deref() {
        if Some(licensed_machine_id) != current_machine_id {
            return Err(LicenseErrorCode::MachineMismatch);
        }
    }

    Ok(LicenseInfo {
        customer_name: payload.customer_name,
        facility_name: payload.facility_name,
        expires_at: payload.expires_at,
    })
}

fn decode_fixed_32(value: &str) -> Result<[u8; 32], LicenseErrorCode> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value.trim())
        .map_err(|_| LicenseErrorCode::InvalidFormat)?;
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| LicenseErrorCode::InvalidFormat)
}
