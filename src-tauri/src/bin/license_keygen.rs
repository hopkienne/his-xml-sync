use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;
use ed25519_dalek::SigningKey;
use his_xml_sync_lib::license_core::{sign_license, LicensePayload};
use std::{env, process};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("keypair") => print_keypair(),
        Some("public") => print_public_key(&args[2..]),
        Some("sign") => sign_from_args(&args[2..]),
        _ => {
            print_usage();
            Ok(())
        }
    }
}

fn print_public_key(args: &[String]) -> Result<(), String> {
    let private_key = required_arg(args, "--private-key")?;
    let private_key_bytes = URL_SAFE_NO_PAD
        .decode(private_key)
        .map_err(|error| error.to_string())?;
    let private_key_bytes: [u8; 32] = private_key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| "Private key phải decode ra đúng 32 bytes".to_string())?;
    let signing_key = SigningKey::from_bytes(&private_key_bytes);
    println!("{}", URL_SAFE_NO_PAD.encode(signing_key.verifying_key().to_bytes()));
    Ok(())
}

fn print_keypair() -> Result<(), String> {
    let mut private_key_bytes = [0_u8; 32];
    getrandom::fill(&mut private_key_bytes).map_err(|error| error.to_string())?;
    let signing_key = SigningKey::from_bytes(&private_key_bytes);
    let verifying_key = signing_key.verifying_key();

    println!(
        "PRIVATE_KEY={}",
        URL_SAFE_NO_PAD.encode(signing_key.to_bytes())
    );
    println!(
        "PUBLIC_KEY={}",
        URL_SAFE_NO_PAD.encode(verifying_key.to_bytes())
    );
    println!();
    println!("Build app production với public key:");
    println!("HIS_XML_LICENSE_PUBLIC_KEY=<PUBLIC_KEY> npm run tauri build");

    Ok(())
}

fn sign_from_args(args: &[String]) -> Result<(), String> {
    let private_key = required_arg(args, "--private-key")?;
    let customer_name = required_arg(args, "--customer")?;
    let facility_name = required_arg(args, "--facility")?;
    let expires_at = required_arg(args, "--expires-at")?;
    let license_id = optional_arg(args, "--license-id")
        .unwrap_or_else(|| format!("LIC-{}", Utc::now().format("%Y%m%d%H%M%S")));
    let machine_id = optional_arg(args, "--machine-id");
    let features = collect_args(args, "--feature");

    let payload = LicensePayload {
        version: 1,
        license_id,
        customer_name,
        facility_name,
        machine_id,
        issued_at: Utc::now().to_rfc3339(),
        expires_at,
        features: if features.is_empty() {
            vec!["xml-sync".to_string()]
        } else {
            features
        },
    };

    let license_key = sign_license(&payload, &private_key).map_err(|error| error.to_string())?;
    println!("{license_key}");
    Ok(())
}

fn required_arg(args: &[String], name: &str) -> Result<String, String> {
    optional_arg(args, name).ok_or_else(|| format!("Thiếu tham số {name}"))
}

fn optional_arg(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|window| window[0] == name)
        .map(|window| window[1].clone())
}

fn collect_args(args: &[String], name: &str) -> Vec<String> {
    args.windows(2)
        .filter(|window| window[0] == name)
        .map(|window| window[1].clone())
        .collect()
}

fn print_usage() {
    eprintln!(
        r#"HIS XML Sync license key generator

Lệnh:
  cargo run --bin license_keygen -- keypair

  cargo run --bin license_keygen -- public --private-key <PRIVATE_KEY>

  cargo run --bin license_keygen -- sign \
    --private-key <PRIVATE_KEY> \
    --customer "Phòng khám demo" \
    --facility "Cơ sở HIS demo" \
    --expires-at "2026-12-31T00:00:00Z" \
    --machine-id "machine-001" \
    --feature "xml-sync"

Ghi chú:
  - PRIVATE_KEY chỉ dùng trong tool sinh key, không nhúng vào app.
  - App production chỉ nhận PUBLIC_KEY qua biến môi trường HIS_XML_LICENSE_PUBLIC_KEY khi build.
  - Bỏ --machine-id nếu muốn license không khóa theo máy.
"#
    );
}
