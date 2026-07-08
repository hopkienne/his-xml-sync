# Thiết kế hệ thống web sinh license key

Tài liệu này mô tả thuật toán license key và cách xây dựng một hệ thống web riêng để sinh key cho ứng dụng desktop **HIS XML Sync**.

## Mục tiêu

- Website nội bộ có thể tạo license key cho từng khách hàng/cơ sở.
- Ứng dụng desktop chỉ xác thực key, không thể tự sinh key.
- Private key chỉ nằm trên hệ thống sinh key, không nhúng vào app desktop.
- Public key được nhúng vào app desktop khi build để verify chữ ký.
- License có ngày hết hạn và có thể khóa theo máy nếu cần.

## Thuật toán tổng quan

Hệ thống dùng chữ ký số Ed25519.

- Website sinh key giữ `PRIVATE_KEY`.
- Desktop app chỉ có `PUBLIC_KEY`.
- Website tạo JSON payload license, ký payload bằng `PRIVATE_KEY`, rồi trả về chuỗi license key.
- Desktop app tách key, verify chữ ký bằng `PUBLIC_KEY`, đọc `expiresAt`, kiểm tra hạn dùng và machine ID.

Điểm quan trọng: không dùng shared secret trong desktop app. Nếu nhúng secret vào app, người khác có thể reverse-engineer để tự sinh key.

## Định dạng license key

```text
HXS1.<payload-base64url>.<signature-base64url>
```

Trong đó:

- `HXS1`: prefix và version của định dạng key.
- `payload-base64url`: JSON payload được encode bằng Base64 URL-safe, không padding.
- `signature-base64url`: chữ ký Ed25519 của bytes JSON payload, encode bằng Base64 URL-safe, không padding.

Ví dụ key:

```text
HXS1.eyJ2ZXJzaW9uIjoxLCJsaWNlbnNlSWQiOiJMSUMtREVNTy0wMDEiLCJjdXN0b21lck5hbWUiOiJQaMOybmcga2jDoW0gZGVtbyJ9.<signature>
```

## Payload contract

Payload là JSON với key theo camelCase:

```json
{
  "version": 1,
  "licenseId": "LIC-202607080001",
  "customerName": "Phòng khám demo",
  "facilityName": "Cơ sở HIS demo",
  "machineId": "machine-001",
  "issuedAt": "2026-07-08T00:00:00Z",
  "expiresAt": "2026-12-31T00:00:00Z",
  "features": ["xml-sync"]
}
```

Ý nghĩa các trường:

| Field | Bắt buộc | Ý nghĩa |
| --- | --- | --- |
| `version` | Có | Version payload. Hiện tại dùng `1`. |
| `licenseId` | Có | Mã license duy nhất trong hệ thống sinh key. |
| `customerName` | Có | Tên khách hàng hiển thị trong app. |
| `facilityName` | Có | Tên cơ sở/phòng khám hiển thị trong app. |
| `machineId` | Không | Nếu có, license chỉ dùng được trên máy có ID này. Nếu `null`, license không khóa theo máy. |
| `issuedAt` | Có | Thời điểm sinh key, định dạng ISO 8601/RFC3339. |
| `expiresAt` | Có | Thời điểm hết hạn, định dạng ISO 8601/RFC3339. |
| `features` | Có | Danh sách feature được bật, ví dụ `["xml-sync"]`. |

## Quy trình tạo cặp khóa

Chạy trong project hiện tại:

```bash
cd /Users/kienth/Tool_Send_KQ_ToHis/his-xml-sync/src-tauri
cargo run --bin license_keygen -- keypair
```

Output:

```text
PRIVATE_KEY=<base64url-32-bytes>
PUBLIC_KEY=<base64url-32-bytes>
```

Quy định lưu trữ:

- `PRIVATE_KEY`: lưu trong secret manager của website sinh key.
- `PUBLIC_KEY`: dùng khi build app desktop.

Ví dụ build app production:

```bash
cd /Users/kienth/Tool_Send_KQ_ToHis/his-xml-sync
HIS_XML_LICENSE_PUBLIC_KEY=<PUBLIC_KEY> npm run tauri build
```

## API đề xuất cho website sinh key

### `POST /api/licenses`

Chỉ cho admin hoặc tài khoản nội bộ có quyền tạo license gọi endpoint này.

Request body:

```json
{
  "customerName": "Phòng khám demo",
  "facilityName": "Cơ sở HIS demo",
  "machineId": "machine-001",
  "expiresAt": "2026-12-31T00:00:00Z",
  "features": ["xml-sync"]
}
```

Response:

```json
{
  "licenseId": "LIC-202607080001",
  "licenseKey": "HXS1.<payload>.<signature>",
  "expiresAt": "2026-12-31T00:00:00Z"
}
```

Validation nên có:

- `customerName` không rỗng.
- `facilityName` không rỗng.
- `expiresAt` phải là thời điểm trong tương lai.
- `features` chỉ được chứa feature hợp lệ.
- `machineId` có thể rỗng nếu không muốn khóa theo máy.

## Pseudo-code TypeScript/Node.js

Có thể triển khai bằng Node.js với package hỗ trợ Ed25519 như `@noble/ed25519`.

```ts
import * as ed from "@noble/ed25519";

type LicensePayload = {
  version: 1;
  licenseId: string;
  customerName: string;
  facilityName: string;
  machineId: string | null;
  issuedAt: string;
  expiresAt: string;
  features: string[];
};

function base64url(input: Uint8Array | string): string {
  const bytes = typeof input === "string" ? Buffer.from(input, "utf8") : Buffer.from(input);
  return bytes.toString("base64url");
}

export async function createLicenseKey(payload: LicensePayload, privateKeyBase64Url: string) {
  const privateKey = Buffer.from(privateKeyBase64Url, "base64url");
  const payloadJson = JSON.stringify(payload);
  const payloadBytes = Buffer.from(payloadJson, "utf8");
  const signature = await ed.sign(payloadBytes, privateKey);

  return `HXS1.${base64url(payloadBytes)}.${base64url(signature)}`;
}
```

Lưu ý: JSON phải được ký đúng bytes mà bạn encode vào key. Không parse rồi stringify lại ở bước verify trước khi verify chữ ký.

## Quy trình verify trong desktop app

Desktop app hiện đã triển khai logic verify trong:

- `src-tauri/src/license_core.rs`
- `src-tauri/src/license.rs`

Quy trình:

1. Trim chuỗi key.
2. Split theo dấu `.` thành 3 phần.
3. Kiểm tra prefix là `HXS1`.
4. Decode payload và signature bằng base64url không padding.
5. Verify signature trên bytes payload bằng `PUBLIC_KEY`.
6. Parse payload JSON.
7. Kiểm tra `version === 1`.
8. Kiểm tra `expiresAt` chưa hết hạn.
9. Nếu payload có `machineId`, so sánh với machine ID hiện tại.
10. Nếu hợp lệ, lưu license key vào local app data.

## Lỗi trả về cho UI

Desktop app đang dùng các mã lỗi sau:

| Code | Ý nghĩa |
| --- | --- |
| `INVALID_FORMAT` | Key sai format, không decode được hoặc payload không hợp lệ. |
| `INVALID_SIGNATURE` | Chữ ký không đúng, payload/signature bị sửa hoặc dùng sai public key. |
| `EXPIRED` | License đã hết hạn. |
| `MACHINE_MISMATCH` | License khóa theo máy khác. |
| `UNKNOWN` | Lỗi ngoài dự kiến. |

Website sinh key nên lưu lại lịch sử request để hỗ trợ debug khi khách báo lỗi.

## Bảo mật production

- Không commit `PRIVATE_KEY`.
- Không log license private key.
- Không trả private key xuống browser client.
- Endpoint sinh key phải chạy server-side.
- Chỉ admin được tạo/revoke license.
- Nên lưu audit log: ai tạo, tạo lúc nào, khách hàng nào, hạn dùng nào.
- Nên rate-limit endpoint tạo key.
- Nên có cơ chế revoke ở hệ thống website, dù desktop app offline hiện chưa kiểm revoke online.
- Nếu muốn revoke realtime, app desktop cần thêm bước gọi API kiểm trạng thái license định kỳ.

## Machine ID

Hiện code app có placeholder đọc machine ID từ biến môi trường `HIS_XML_MACHINE_ID`. Trước khi khóa license theo máy trong production, cần thay bằng cách lấy định danh máy ổn định trên Windows.

Gợi ý:

- Dùng Windows MachineGuid hoặc hardware fingerprint có kiểm soát.
- Không dùng MAC address duy nhất vì có thể thay đổi khi đổi card mạng/VPN.
- Cho phép admin reset machine ID khi khách đổi máy.

Nếu chưa cần khóa theo máy, hãy để `machineId: null`.

## Checklist triển khai website sinh key

1. Tạo cặp Ed25519 keypair.
2. Lưu `PRIVATE_KEY` vào secret manager của website.
3. Build app desktop bằng `PUBLIC_KEY`.
4. Tạo database table `licenses`.
5. Tạo endpoint `POST /api/licenses`.
6. Validate input.
7. Sinh `licenseId`.
8. Tạo payload.
9. Ký payload bằng private key.
10. Trả license key cho admin.
11. Lưu audit log.
12. Test license key bằng app desktop trước khi giao khách.

## Test tương thích với Rust implementation

Sau khi website sinh key, hãy lấy key đó paste vào app desktop chạy bằng:

```bash
cd /Users/kienth/Tool_Send_KQ_ToHis/his-xml-sync
npm run tauri dev
```

Nếu app báo `INVALID_SIGNATURE`, kiểm tra:

- Website có đang dùng đúng private key tương ứng với public key đã build app không.
- Payload bytes dùng để ký có đúng là payload bytes encode vào key không.
- Base64 có phải URL-safe không padding không.
- App desktop đã được rebuild sau khi thay `HIS_XML_LICENSE_PUBLIC_KEY` chưa.

## Lệnh CLI tham chiếu

Sinh public key từ private key:

```bash
cd /Users/kienth/Tool_Send_KQ_ToHis/his-xml-sync/src-tauri
cargo run --bin license_keygen -- public --private-key <PRIVATE_KEY>
```

Sinh license:

```bash
cargo run --bin license_keygen -- sign \
  --private-key <PRIVATE_KEY> \
  --customer "Phòng khám demo" \
  --facility "Cơ sở HIS demo" \
  --expires-at "2026-12-31T00:00:00Z" \
  --feature "xml-sync"
```
