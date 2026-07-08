# Thuật toán license key

> Tài liệu này mô tả thuật toán trong app/CLI Rust. Nếu cần xây dựng website sinh key riêng, xem thêm `docs/license-keygen-web-system.md`.

## Mục tiêu

Ứng dụng desktop chỉ được phép xác thực license, không được chứa secret có thể dùng để sinh key. Tool sinh key chạy riêng và giữ private key.

## Cấu trúc key

License key có định dạng:

```text
HXS1.<payload-base64url>.<signature-base64url>
```

`payload` là JSON:

```json
{
  "version": 1,
  "licenseId": "LIC-DEMO-001",
  "customerName": "Phòng khám demo",
  "facilityName": "Cơ sở HIS demo",
  "machineId": "machine-001",
  "issuedAt": "2026-07-08T00:00:00Z",
  "expiresAt": "2026-12-31T00:00:00Z",
  "features": ["xml-sync"]
}
```

`signature` là chữ ký Ed25519 của bytes JSON payload.

## Quy trình sinh key

1. Tạo cặp key:

```bash
cd /Users/kienth/Tool_Send_KQ_ToHis/his-xml-sync/src-tauri
cargo run --bin license_keygen -- keypair
```

2. Build app production với public key:

```bash
HIS_XML_LICENSE_PUBLIC_KEY=<PUBLIC_KEY> npm run tauri build
```

3. Sinh license bằng private key:

```bash
cd /Users/kienth/Tool_Send_KQ_ToHis/his-xml-sync/src-tauri
cargo run --bin license_keygen -- sign \
  --private-key <PRIVATE_KEY> \
  --customer "Phòng khám demo" \
  --facility "Cơ sở HIS demo" \
  --expires-at "2026-12-31T00:00:00Z" \
  --machine-id "machine-001" \
  --feature "xml-sync"
```

## Quy trình xác thực trong app

1. Tách key thành 3 phần.
2. Decode payload và signature bằng base64url không padding.
3. Verify chữ ký bằng public key nhúng khi build.
4. Parse payload JSON.
5. Kiểm tra `version`.
6. Kiểm tra `expiresAt` so với thời gian hiện tại.
7. Nếu payload có `machineId`, kiểm tra trùng máy hiện tại.
8. Nếu hợp lệ, lưu key vào local app data và trả thông tin license cho UI.

## Lưu ý production

- Không commit private key.
- Không nhúng private key vào app.
- Public key có thể nằm trong binary app.
- Cần thay hàm lấy machine ID hiện tại bằng cách đọc định danh máy ổn định trên Windows trước khi khóa license theo máy.
