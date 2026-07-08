use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{TimeZone, Utc};
use ed25519_dalek::SigningKey;
use his_xml_sync_lib::license_core::{
    sign_license, verify_license, LicenseErrorCode, LicensePayload,
};

fn test_keys() -> (String, String) {
    let seed = [7_u8; 32];
    let signing_key = SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();

    (
        URL_SAFE_NO_PAD.encode(signing_key.to_bytes()),
        URL_SAFE_NO_PAD.encode(verifying_key.to_bytes()),
    )
}

fn dev_app_keys() -> (String, String) {
    let seed = [7_u8; 32];
    let signing_key = SigningKey::from_bytes(&seed);
    (
        URL_SAFE_NO_PAD.encode(signing_key.to_bytes()),
        "6kpsY-KcUgq-9VB7Ey7F-ZVHdq6-vnuSQh7qaRRG0iw".to_string(),
    )
}

fn valid_payload() -> LicensePayload {
    LicensePayload {
        version: 1,
        license_id: "LIC-DEMO-001".to_string(),
        customer_name: "Phòng khám demo".to_string(),
        facility_name: "Cơ sở HIS demo".to_string(),
        machine_id: Some("machine-001".to_string()),
        issued_at: "2026-07-08T00:00:00Z".to_string(),
        expires_at: "2026-12-31T00:00:00Z".to_string(),
        features: vec!["xml-sync".to_string()],
    }
}

#[test]
fn signed_license_verifies_and_returns_display_info() {
    let (private_key, public_key) = test_keys();
    let license_key = sign_license(&valid_payload(), &private_key).expect("license signs");

    assert!(license_key.starts_with("HXS1."));

    let info = verify_license(
        &license_key,
        &public_key,
        Some("machine-001"),
        Utc.with_ymd_and_hms(2026, 7, 8, 0, 0, 0).unwrap(),
    )
    .expect("license verifies");

    assert_eq!(info.customer_name, "Phòng khám demo");
    assert_eq!(info.facility_name, "Cơ sở HIS demo");
    assert_eq!(info.expires_at, "2026-12-31T00:00:00Z");
}

#[test]
fn expired_license_is_rejected() {
    let (private_key, public_key) = test_keys();
    let license_key = sign_license(&valid_payload(), &private_key).expect("license signs");

    let error = verify_license(
        &license_key,
        &public_key,
        Some("machine-001"),
        Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 1).unwrap(),
    )
    .expect_err("license must be expired");

    assert_eq!(error, LicenseErrorCode::Expired);
}

#[test]
fn tampered_license_payload_is_rejected() {
    let (private_key, public_key) = test_keys();
    let license_key = sign_license(&valid_payload(), &private_key).expect("license signs");
    let mut parts: Vec<&str> = license_key.split('.').collect();
    parts[1] = "eyJ2ZXJzaW9uIjoyfQ";
    let tampered_key = parts.join(".");

    let error = verify_license(
        &tampered_key,
        &public_key,
        Some("machine-001"),
        Utc.with_ymd_and_hms(2026, 7, 8, 0, 0, 0).unwrap(),
    )
    .expect_err("tampered payload must fail signature verification");

    assert_eq!(error, LicenseErrorCode::InvalidSignature);
}

#[test]
fn machine_mismatch_is_rejected() {
    let (private_key, public_key) = test_keys();
    let license_key = sign_license(&valid_payload(), &private_key).expect("license signs");

    let error = verify_license(
        &license_key,
        &public_key,
        Some("other-machine"),
        Utc.with_ymd_and_hms(2026, 7, 8, 0, 0, 0).unwrap(),
    )
    .expect_err("license must be tied to a different machine");

    assert_eq!(error, LicenseErrorCode::MachineMismatch);
}

#[test]
fn dev_private_key_matches_app_fallback_public_key() {
    let (private_key, public_key) = dev_app_keys();
    let license_key = sign_license(&valid_payload(), &private_key).expect("license signs");

    let info = verify_license(
        &license_key,
        &public_key,
        Some("machine-001"),
        Utc.with_ymd_and_hms(2026, 7, 8, 0, 0, 0).unwrap(),
    )
    .expect("dev key must verify with app fallback public key");

    assert_eq!(info.facility_name, "Cơ sở HIS demo");
}
