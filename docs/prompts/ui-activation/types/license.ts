/**
 * Hợp đồng dữ liệu (contract) giữa frontend và Tauri command cho tính năng
 * kích hoạt license. Giữ nguyên các kiểu này khi thay phần logic Rust thật
 * để không phải sửa lại UI.
 */

/** Các mã lỗi mà command `activate_license` có thể trả về. */
export type LicenseErrorCode =
  | 'INVALID_FORMAT' // Key sai định dạng
  | 'INVALID_SIGNATURE' // Chữ ký license không hợp lệ
  | 'EXPIRED' // License đã hết hạn
  | 'MACHINE_MISMATCH' // License không khớp với máy hiện tại
  | 'UNKNOWN'; // Lỗi không xác định (mạng, panic, ...)

export interface LicenseError {
  code: LicenseErrorCode;
  message?: string;
}

/** Thông tin license hợp lệ, dùng để hiển thị tóm tắt sau khi kích hoạt. */
export interface LicenseInfo {
  customerName: string;
  facilityName: string;
  /** ISO 8601, ví dụ "2026-12-31T00:00:00Z" */
  expiresAt: string;
}

/** Kết quả của command `get_license_status`, gọi khi ứng dụng khởi động. */
export interface LicenseStatus {
  valid: boolean;
  info?: LicenseInfo;
}
