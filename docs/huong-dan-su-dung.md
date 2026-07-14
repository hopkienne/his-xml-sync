# Hướng dẫn sử dụng HIS XML Sync

**HIS XML Sync** giúp đưa kết quả đo khúc xạ từ máy **TOPCON KR-800** (file XML) lên hệ thống **HIS** của phòng khám / bệnh viện.

Bạn chỉ cần làm 2 việc chính:

1. **Cấu hình** kết nối HIS (một lần hoặc khi đổi tài khoản).
2. **Chọn thư mục** chứa file XML và bấm **Xử lý**.

---

## 1. Giao diện tổng quan

Khi mở app, màn hình chia làm 2 phần:

| Vùng | Ý nghĩa |
|------|---------|
| **Menu bên trái** | Chuyển giữa các chức năng |
| **Nội dung chính** | Thao tác và xem kết quả |

### Menu bên trái

- **KR-800** — Màn hình làm việc hằng ngày: theo dõi file XML và gửi kết quả lên HIS.
- **Cấu hình** — Thiết lập kết nối HIS, xem phiên đăng nhập, xuất nhật ký khi cần hỗ trợ kỹ thuật.

### Góc trên bên phải

- Tên **cơ sở** (ví dụ: *Cơ sở HIS demo*).
- Trạng thái đăng nhập HIS: **HIS: chưa login** hoặc đã đăng nhập.
- Nút **Đổi key** — dùng khi cần đổi license key của app.

> **Lưu ý:** Muốn gửi dữ liệu lên HIS, app phải **đã login HIS** thành công.

---

## 2. Bước 1 — Cấu hình kết nối HIS

Vào menu **Cấu hình** (bên trái).

### 2.1. Điền thông tin kết nối

Trong phần **Cấu hình kết nối**, nhập:

| Ô nhập | Điền gì? |
|--------|----------|
| **API URL HIS (base)** | Địa chỉ máy chủ HIS, ví dụ: `https://hisvn.vietngagroup.vn` |
| **Tài khoản (taiKhoan)** | Tài khoản API do bộ phận IT / HIS cấp |
| **Mật khẩu (matKhau)** | Mật khẩu tương ứng (hiển thị dạng chấm tròn) |
| **Cơ sở khám bệnh ID** | Mã số cơ sở trên HIS (ví dụ: `4`) |

### 2.2. Tùy chọn “Sao chép kết quả khúc xạ sang kính mới”

- **Bỏ trống (mặc định):** chỉ gửi kết quả đo khúc xạ theo quy trình chuẩn.
- **Tích chọn:** app sẽ sao chép kết quả khúc xạ sang phần **kính mới** trên HIS (chỉ bật khi phòng khám yêu cầu).

### 2.3. Lưu và đăng nhập

Có 2 nút:

| Nút | Khi nào dùng |
|-----|----------------|
| **Kiểm tra / Login HIS** | Thử đăng nhập để xem thông tin đã đúng chưa (chưa bắt buộc lưu). |
| **Lưu & Login** | **Nên dùng hằng ngày / lần đầu:** lưu cấu hình rồi đăng nhập HIS. |

Sau khi thành công:

- Góc phải hiển thị trạng thái đã kiểm tra / đã có phiên đăng nhập.
- Phần **Phiên đăng nhập HIS** hiện **Đã login**, tên user, cơ sở, thời hạn token, v.v.

Nếu **Chưa login** hoặc báo lỗi:

1. Kiểm tra lại URL, tài khoản, mật khẩu, ID cơ sở.
2. Kiểm tra máy có vào được mạng nội bộ / HIS không.
3. Bấm lại **Lưu & Login**.

### 2.4. Phiên đăng nhập HIS (chỉ để xem)

Khu vực này cho biết app **đang đăng nhập HIS hay chưa**:

- **Trạng thái:** Chưa login / Đã login
- **User:** người dùng API
- **coSoKcbId:** cơ sở đang dùng
- **Hết hạn token:** thời điểm phiên đăng nhập hết hạn
- **Có access_token:** Có / Không

Bạn **không cần** nhớ token. App tự quản lý; token **không** hiện đầy đủ trên màn hình để bảo mật.

### 2.5. Nhật ký ứng dụng (khi cần hỗ trợ)

Phần **Nhật ký ứng dụng** ghi lại các request/response (không lưu mật khẩu / token đầy đủ) để kỹ thuật viên kiểm tra lỗi.

- Bấm **Xuất logs** khi IT / hỗ trợ yêu cầu gửi file log.
- Có thể xem đường dẫn file log và dung lượng file.

---

## 3. Bước 2 — Xử lý file XML từ máy KR-800

Vào menu **KR-800**.

Đây là màn hình làm việc chính sau khi đã cấu hình và login HIS.

### 3.1. Chọn thư mục tracking

1. Bấm **Chọn thư mục**.
2. Chọn thư mục máy TOPCON KR-800 **xuất file XML** (ví dụ: `Tool_Send_KQ_ToHis`).
3. App sẽ quét các file `.xml` trong thư mục đó.

Sau khi chọn xong:

- Đường dẫn thư mục hiển thị ở **Thư mục tracking**.
- Các chip thống kê: **Trong khoảng**, **Chờ**, **Lỗi**, …

**Quét lại:** dùng khi máy vừa xuất thêm file XML mới mà danh sách chưa cập nhật.

### 3.2. Chọn khoảng thời gian xử lý

Ở dòng **Ngày xử lý**:

- Chọn **Từ** ngày/giờ → **Đến** ngày/giờ.
- Danh sách file chỉ hiện những file có **ngày tạo** nằm trong khoảng này.

Ví dụ: từ `06/07/2026 00:00` đến `10/07/2026 23:59`.

> Nếu bảng trống nhưng bạn biết có file: mở rộng khoảng ngày, hoặc bấm **Quét lại**.

### 3.3. Xem danh sách file XML

Bảng gồm:

| Cột | Ý nghĩa |
|-----|---------|
| **Tên file** | Tên file XML từ máy đo |
| **Kích thước** | Dung lượng file |
| **Trạng thái** | File đang ở bước nào (xem mục 4) |
| **Ngày tạo** | Thời điểm file được tạo |
| **Ngày cập nhật** | Lần app cập nhật trạng thái gần nhất |
| **Lỗi** | Chi tiết lỗi (nếu có) |

### 3.4. Bấm “Xử lý”

1. Đảm bảo **đã login HIS** (góc trên phải không còn “chưa login”).
2. Đã chọn thư mục tracking.
3. Đã chọn đúng khoảng **Ngày xử lý**.
4. Bấm nút xanh **Xử lý**.

App sẽ lần lượt:

1. Đọc file XML.
2. Tìm bệnh nhân / đợt điều trị tương ứng trên HIS.
3. Mapping danh mục và gửi kết quả lên HIS.

Trong lúc chạy, nút hiển thị **Đang xử lý…** — vui lòng chờ, không tắt app.

Khi xong, app báo tóm tắt kiểu:

- Đã xử lý bao nhiêu file
- Bỏ qua file trùng
- Bao nhiêu file lỗi

---

## 4. Ý nghĩa các trạng thái file

| Trạng thái trên màn hình | Nghĩa đơn giản |
|--------------------------|----------------|
| **Chờ xử lý** | File mới, chưa gửi lên HIS |
| **Đang xử lý** | App đang làm việc với file này |
| **Đã phân tích XML** | Đã đọc được nội dung file |
| **Đã tìm thấy bệnh nhân** | Khớp được bệnh nhân trên HIS |
| **Đã mapping danh mục** | Đã map dữ liệu đo sang mã HIS |
| **Đang gửi HIS** | Đang đẩy kết quả lên server |
| **Đã xử lý** | **Thành công** — kết quả đã lên HIS |
| **Không tìm thấy bệnh nhân** | Mã BN / thông tin trong XML không khớp HIS |
| **Không xác định đợt điều trị** | Tìm thấy BN nhưng không rõ đợt khám / điều trị |
| **Lỗi XML** | File hỏng, sai định dạng, hoặc đọc không được |
| **Lỗi mapping danh mục** | Không map được chỉ số đo sang danh mục HIS |
| **Lỗi gửi HIS** | Mạng / API HIS từ chối hoặc lỗi khi gửi |
| **Thất bại** | Xử lý không thành công (xem cột **Lỗi**) |

**Mục tiêu mong muốn:** file chuyển sang **Đã xử lý**, cột **Lỗi** trống.

---

## 5. Quy trình làm việc gợi ý mỗi ngày

```text
1. Mở app HIS XML Sync
2. Kiểm tra góc trên phải: đã login HIS chưa
   → Nếu chưa: vào Cấu hình → Lưu & Login
3. Vào KR-800
4. Chọn / kiểm tra đúng thư mục XML của máy đo
5. Bấm Quét lại (nếu vừa đo xong)
6. Chọn khoảng Ngày xử lý phù hợp (thường là hôm nay)
7. Bấm Xử lý
8. Xem cột Trạng thái:
   - Đã xử lý → OK
   - Lỗi / Không tìm thấy bệnh nhân → xử lý theo mục 6
```

---

## 6. Xử lý sự cố thường gặp

### “HIS: chưa login”

- Vào **Cấu hình** → kiểm tra URL, tài khoản, mật khẩu, ID cơ sở.
- Bấm **Lưu & Login**.
- Kiểm tra mạng nội bộ / VPN nếu HIS chỉ mở trong viện.

### Bảng file trống

- Đã **Chọn thư mục** đúng chỗ máy xuất XML chưa?
- Bấm **Quét lại**.
- Mở rộng **Ngày xử lý** (từ–đến).
- Kiểm tra thư mục có file `.xml` thật không (mở Explorer / Finder).

### File “Không tìm thấy bệnh nhân”

- Kiểm tra bệnh nhân đã có trên HIS chưa.
- Mã bệnh nhân trong XML (thường gắn theo quy trình đo) có đúng với HIS không.
- Khoảng **Ngày xử lý** có bao phủ thời điểm bệnh nhân vào viện / khám không.

### File “Lỗi gửi HIS” hoặc “Thất bại”

- Xem cột **Lỗi** trên bảng.
- Thử **Lưu & Login** lại rồi **Xử lý** lại.
- Nếu vẫn lỗi: **Cấu hình** → **Xuất logs** → gửi file cho IT / hỗ trợ kỹ thuật.

### Nút “Xử lý” bị mờ, không bấm được

Có thể do:

- Chưa chọn thư mục tracking
- Đang quét / đang tải danh sách
- Đang xử lý lượt trước
- Khoảng thời gian “Từ” > “Đến” (không hợp lệ)
- App đang bận thao tác login HIS khác

Sửa điều kiện trên rồi thử lại.

---

## 7. Lưu ý an toàn & vận hành

- **Không chia sẻ** mật khẩu HIS và license key.
- Chỉ cấu hình trên máy làm việc của phòng đo / IT được giao.
- App lưu cấu hình cục bộ; khi đổi mật khẩu HIS, vào **Cấu hình** và **Lưu & Login** lại.
- Nên để app mở trong ca làm việc nếu thường xuyên có file XML mới; sau mỗi đợt đo, **Quét lại** → **Xử lý**.

---

## 8. Tóm tắt nhanh (1 phút)

| Việc cần làm | Ở đâu | Nút chính |
|--------------|--------|-----------|
| Kết nối HIS lần đầu / đổi TK | **Cấu hình** | **Lưu & Login** |
| Chọn chỗ chứa file XML | **KR-800** | **Chọn thư mục** |
| Cập nhật file mới | **KR-800** | **Quét lại** |
| Gửi kết quả lên HIS | **KR-800** | **Xử lý** |
| Gửi log khi lỗi | **Cấu hình** | **Xuất logs** |

---

*Tài liệu dành cho nhân viên vận hành phòng khám. Nếu cần cấu hình server, license hoặc cài đặt máy, liên hệ bộ phận IT / nhà cung cấp.*
