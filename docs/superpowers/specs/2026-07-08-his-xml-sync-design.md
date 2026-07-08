# HIS XML Sync Design

## Goal

Build a Windows-first desktop tool that reads ophthalmology XML files from a selected folder, maps refraction values to HIS payload IDs, and sends results to the HIS API. The app is developed on macOS but must run on Windows and macOS.

## Stack

- Tauri v2 desktop shell
- React 19 + TypeScript + Vite frontend
- Rust backend commands for local filesystem, license verification, XML parsing, sync orchestration, and HIS API calls

## Activation Model

The production license should use asymmetric signing. A separate license generator keeps a private Ed25519 key. The app embeds only the public key. The activation key contains a signed payload with customer metadata, machine scope, feature flags, and `expiresAt`. The app verifies the signature locally and reads the expiry date from the payload. If the license is missing, invalid, or expired, the user returns to the activation screen.

## Desktop Behavior

The main window close button hides the app instead of quitting. The app remains available from the Windows/macOS tray. The tray menu contains show, sync now, and quit actions.

## Main Screens

- Activation screen: enter/paste license key and display validation errors.
- Home shell: left sidebar with dashboard, HIS settings, XML folder, sync, logs, and license sections.

## Backend Boundaries

- `license`: verify signed activation key and expose expiry status.
- `settings`: persist local settings such as HIS base URL, account, facility, and XML folder.
- `xml_parser`: parse TOPCON XML data and extract patient ID, measured time, and R/L sphere/cylinder/axis values.
- `his_api`: login, fetch patients, and submit refraction payloads.
- `sync`: coordinate folder scanning, XML parsing, patient matching, payload mapping, API submission, and logging.
- `tray`: manage system tray and close-to-hide behavior.
