# HIS XML Sync

Desktop app (Tauri v2 + React + TypeScript) đồng bộ kết quả khúc xạ XML (TOPCON KR-800) lên HIS.

- **Windows-first**: user cài bằng file `*-setup.exe` (NSIS), không cần Node/Rust.
- **Dev trên macOS** được; **build installer Windows** qua GitHub Actions (`windows-latest`).

## Development

Yêu cầu: Node.js LTS, Rust stable, (Windows) WebView2.

```bash
npm install
npm run tauri:dev
```

## Production build (local)

Public key license **bắt buộc** khi build bản phát hành (nhúng vào binary lúc compile):

```bash
# macOS / Linux
HIS_XML_LICENSE_PUBLIC_KEY=<PUBLIC_KEY> npm run tauri:build

# Windows (PowerShell)
$env:HIS_XML_LICENSE_PUBLIC_KEY="<PUBLIC_KEY>"
npm run tauri:build:windows
```

Sinh keypair:

```bash
cd src-tauri
cargo run --bin license_keygen -- keypair
```

Chi tiết thuật toán: [`docs/license-algorithm.md`](docs/license-algorithm.md).

### Output trên Windows

```text
src-tauri/target/release/bundle/nsis/HIS XML Sync_<version>_x64-setup.exe
```

Installer đóng gói toàn bộ app (frontend + Rust backend + WebView2 bootstrap nếu thiếu). User chỉ cần chạy setup.

> **Không** cross-compile Windows từ macOS. Muốn ra `.exe` cài được → build trên máy Windows hoặc CI `windows-latest`.

## Release chuẩn (GitHub Actions)

Pipeline: [`.github/workflows/release-windows.yml`](.github/workflows/release-windows.yml)

### 1. Cấu hình secret (một lần)

GitHub repo → **Settings → Secrets and variables → Actions → New repository secret**

| Secret | Giá trị |
|--------|---------|
| `HIS_XML_LICENSE_PUBLIC_KEY` | Public key base64url (output của `license_keygen keypair`) |

> Private key **không** commit, **không** đưa vào Actions secrets dùng build app.

### 2. Chọn version release

Khi chạy tay, chọn **Actions → Release Windows → Run workflow**, rồi nhập `version` theo SemVer, không kèm tiền tố `v` (ví dụ `1.2.3` hoặc `1.2.3-rc.1`). Pipeline tự đồng bộ version này vào app trước khi build; tên installer và draft GitHub Release cũng dùng đúng version đã nhập.

Khi phát hành bằng tag, vẫn cập nhật cùng version semver trong source trước:

- `package.json` → `version`
- `src-tauri/tauri.conf.json` → `version`
- `src-tauri/Cargo.toml` → `version`

### 3. Tag và push

```bash
git add -A
git commit -m "Release v0.1.0"
git tag v0.1.0
git push origin main
git push origin v0.1.0
```

Hoặc chạy tay theo bước 2, không cần commit thay đổi version chỉ để tạo bản build đó.

### 4. Lấy file cài đặt

1. Mở **Actions** → run vừa xong → artifact `his-xml-sync-windows-nsis`, **hoặc**
2. Mở **Releases** → draft **HIS XML Sync vX.Y.Z** → tải `*_x64-setup.exe` → **Publish release** khi đã kiểm tra xong.

### 5. Cài trên máy user

1. Tải `HIS XML Sync_*_x64-setup.exe`
2. Chạy installer (shortcut Desktop + Start Menu)
3. Mở app → nhập license key đã ký bằng private key tương ứng public key lúc build

## Bundle config

`src-tauri/tauri.conf.json`:

- `bundle.targets`: `["nsis"]` — installer Windows thân thiện
- `webviewInstallMode`: `downloadBootstrapper` — tự bootstrap WebView2 nếu máy chưa có
- `nsis.installMode`: `currentUser` — cài per-user (không cần admin trong hầu hết trường hợp)

Code signing Windows (chứng chỉ Authenticode) có thể bổ sung sau để giảm cảnh báo SmartScreen.

## License tools

```bash
cd src-tauri
cargo run --bin license_keygen -- keypair
cargo run --bin license_keygen -- sign \
  --private-key <PRIVATE_KEY> \
  --customer "Phòng khám demo" \
  --facility "Cơ sở HIS demo" \
  --expires-at "2026-12-31T00:00:00Z" \
  --feature "xml-sync"
```
