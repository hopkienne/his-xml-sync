# Prompt: Xây dựng UI màn hình chính

Bạn là agent phụ trách xây dựng UI cho ứng dụng desktop dùng Tauri v2, React, TypeScript và Vite. Hãy thiết kế và triển khai màn hình chính cho ứng dụng **HIS XML Sync**.

## Bối cảnh sản phẩm

Ứng dụng tự động phân tích các file XML xuất ra từ máy đo khúc xạ TOPCON KR-800 trong thư mục được chọn, đối chiếu kết quả với người bệnh trên HIS và gửi payload kết quả lên API HIS. Khi người dùng bấm nút đóng cửa sổ, ứng dụng không thoát hẳn mà tiếp tục chạy ẩn dưới system tray.

## Layout chính

- Sidebar nằm bên trái, có chiều rộng ổn định.
- Khu vực bên phải hiển thị nội dung tương ứng với menu đang được chọn.
- Không xây dựng hero section hoặc landing page.
- Giao diện cần giống một công cụ vận hành nội bộ: dễ quét trạng thái, thao tác nhanh, ít trang trí, ưu tiên tính rõ ràng và độ tin cậy.

## Menu sidebar

1. Tổng quan
   - Hiển thị trạng thái license, kết nối HIS, thư mục XML và lần đồng bộ gần nhất.
   - Có các card thống kê: file chờ xử lý, đã gửi hôm nay, cần kiểm tra, lỗi.
2. Cấu hình HIS
   - Cho phép cấu hình Base URL, tài khoản, mật khẩu hoặc chính sách token, `coSoKcbId`, phòng/khoa nếu cần.
   - Có nút kiểm tra kết nối.
3. Thư mục XML
   - Cho phép chọn thư mục.
   - Hiển thị đường dẫn thư mục, số lượng file XML tìm thấy và file mới nhất.
   - Có tùy chọn bật/tắt tự động đồng bộ.
4. Đồng bộ
   - Có nút **Đồng bộ ngay**.
   - Có bảng danh sách file gồm: tên file, patient ID, thời gian đo, trạng thái, lỗi nếu có.
   - Có khu vực preview kết quả mắt phải/mắt trái gồm sphere, cylinder, axis trước khi gửi nếu cần.
5. Nhật ký
   - Cho phép lọc theo ngày và trạng thái.
   - Hiển thị log dưới dạng timeline hoặc table.
6. License
   - Hiển thị khách hàng/cơ sở sử dụng, ngày hết hạn, trạng thái license và nút đổi key.

## Ràng buộc kỹ thuật

- Sử dụng React functional components và TypeScript.
- Dùng state hoặc routing đơn giản cho menu. Chưa cần thêm `react-router` nếu ứng dụng chưa có routing phức tạp.
- Gọi các Tauri command hiện có:
  - `get_license_status`
  - `get_settings`
  - `save_settings`
  - `preview_xml_file`
  - `run_sync_once`
- Nếu cần folder picker, hãy để placeholder rõ contract trước khi thêm plugin dialog.
- Nên tổ chức component theo hướng:
  - `src/screens/HomeShell.tsx`
  - `src/components/Sidebar.tsx`
  - `src/components/StatCard.tsx`
  - `src/features/settings/HisSettingsPanel.tsx`
  - `src/features/xml-folder/XmlFolderPanel.tsx`
  - `src/features/sync/SyncPanel.tsx`
  - `src/features/logs/LogsPanel.tsx`
  - `src/features/license/LicensePanel.tsx`

## Hướng dẫn thiết kế

- Ưu tiên mật độ thông tin vừa phải để nhân viên phòng khám thao tác nhanh.
- Card dùng border radius tối đa 8px.
- Không đặt card lồng trong card.
- Không dùng palette quá đơn sắc. Nên có màu trung tính kết hợp với teal, blue, amber và red cho các trạng thái khác nhau.
- Nếu đã có `lucide-react`, nút thao tác nên dùng icon phù hợp như `Folder`, `RefreshCw`, `UploadCloud`, `Settings`, `Key`, `AlertCircle`.
- Văn bản trong table, button và card không được bị overflow khi cửa sổ nhỏ.

## Đầu ra mong muốn

Triển khai home shell có các panel chạy được với dữ liệu mock hoặc local state, nhưng mỗi action cần được đặt đúng vị trí để sau này nối với Tauri command thật. Giao diện cần sẵn sàng cho bước tiếp theo: parse XML thật, map danh mục ID và gửi dữ liệu lên API HIS.
