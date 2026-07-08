# Prompt: Xây dựng UI màn hình nhập key kích hoạt

Bạn là agent phụ trách xây dựng UI cho ứng dụng desktop dùng Tauri v2, React, TypeScript và Vite. Hãy thiết kế và triển khai màn hình nhập key kích hoạt cho ứng dụng **HIS XML Sync**.

## Bối cảnh sản phẩm

Ứng dụng dùng để đọc file XML xuất ra từ máy đo khúc xạ TOPCON KR-800, phân tích dữ liệu và gửi kết quả lên hệ thống HIS. Ứng dụng chủ yếu chạy trên Windows, nhưng quá trình phát triển hiện tại được thực hiện trên macOS. Khi license hết hạn, ứng dụng phải tự động quay lại màn hình nhập key kích hoạt mới.

## Yêu cầu UI

- Đây là màn hình đầu tiên khi ứng dụng khởi động nếu chưa có license hợp lệ.
- Giao diện cần mang cảm giác của một công cụ vận hành nội bộ trong môi trường y tế: rõ ràng, tin cậy, gọn gàng, không mang phong cách landing page hoặc marketing.
- Hiển thị một panel nhập key ở giữa màn hình, có tên sản phẩm **HIS XML Sync**.
- Có textarea để người dùng dán key dài.
- Có nút chính với nhãn **Kích hoạt**.
- Có trạng thái loading khi đang xác thực key.
- Hiển thị lỗi rõ ràng cho các trường hợp:
  - Key sai định dạng.
  - Chữ ký license không hợp lệ.
  - License đã hết hạn.
  - License không khớp với máy hiện tại.
- Nếu kích hoạt thành công, hiển thị tóm tắt ngắn gồm khách hàng/cơ sở sử dụng và ngày hết hạn trước khi chuyển vào màn hình chính.
- Có hành động phụ **Nhập key khác** khi xác thực thất bại.
- Toàn bộ nội dung hiển thị bằng tiếng Việt, ngắn gọn, dễ hiểu với nhân sự phòng khám.

## Ràng buộc kỹ thuật

- Sử dụng React functional components và TypeScript.
- Gọi Tauri command `activate_license` để xác thực key.
- Gọi Tauri command `get_license_status` khi component được mount để tự động chuyển vào màn hình chính nếu license hiện tại vẫn còn hạn.
- Không hard-code license hợp lệ ở frontend.
- Có thể tách component thành các file nhỏ nếu cần:
  - `src/screens/ActivationScreen.tsx`
  - `src/components/LicenseKeyInput.tsx`
  - `src/components/StatusMessage.tsx`
- State tối thiểu cần có: key, loading, error, thông tin license đã xác thực.
- CSS phải responsive tốt khi cửa sổ ứng dụng bị thu nhỏ trên Windows.

## Hướng dẫn thiết kế

- Nên dùng nền trắng, xám lạnh hoặc xanh xám rất nhẹ. Tránh dùng gradient tím/xanh đậm làm màu chủ đạo.
- Card/panel dùng border radius tối đa 8px.
- Nút chính nên dùng màu teal, xanh lá y tế hoặc xanh dương nghiệp vụ.
- Văn bản không được tràn khỏi button, panel hoặc vùng nhập liệu.
- Không thêm icon nếu chưa có icon library. Nếu thêm `lucide-react`, hãy dùng các icon quen thuộc như `Key`, `Shield`, `Check`, `AlertCircle`.

## Đầu ra mong muốn

Triển khai hoàn chỉnh activation flow và kết nối đúng với command contract hiện có. Nếu backend command vẫn đang là stub, hãy giữ cách xử lý theo đúng contract để sau này thay logic Rust thật mà không phải sửa lại UI.
