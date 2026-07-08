import { AlertCircle, Check } from "lucide-react";
import type { LicenseErrorCode, LicenseInfo } from "../types/license";

const ERROR_COPY: Record<LicenseErrorCode, string> = {
  INVALID_FORMAT: "Key sai định dạng. Vui lòng kiểm tra lại và dán đầy đủ nội dung key.",
  INVALID_SIGNATURE: "Chữ ký license không hợp lệ. Key này có thể đã bị chỉnh sửa hoặc không đúng.",
  EXPIRED: "License đã hết hạn. Vui lòng liên hệ quản trị để được cấp key mới.",
  MACHINE_MISMATCH: "License không khớp với máy này. Key chỉ dùng được trên máy đã đăng ký.",
  RUNTIME_UNAVAILABLE:
    "Bạn đang mở giao diện bằng browser nên không thể gọi Tauri command để xác thực key. Vui lòng chạy ứng dụng bằng `npm run tauri dev` hoặc mở bản desktop.",
  UNKNOWN: "Không thể xác thực key. Vui lòng thử lại hoặc liên hệ quản trị hệ thống.",
};

function formatExpiry(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleDateString("vi-VN", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
  });
}

interface ErrorStatusProps {
  variant: "error";
  code: LicenseErrorCode;
}

interface SuccessStatusProps {
  variant: "success";
  info: LicenseInfo;
}

type StatusMessageProps = ErrorStatusProps | SuccessStatusProps;

export function StatusMessage(props: StatusMessageProps) {
  if (props.variant === "error") {
    return (
      <div className="status-message status-message--error" role="alert">
        <AlertCircle size={18} strokeWidth={2} aria-hidden="true" />
        <p>{ERROR_COPY[props.code]}</p>
      </div>
    );
  }

  const { info } = props;
  return (
    <div className="status-message status-message--success" role="status">
      <div className="status-message__header">
        <Check size={18} strokeWidth={2} aria-hidden="true" />
        <p>Kích hoạt thành công</p>
      </div>
      <dl className="status-message__details">
        <div>
          <dt>Cơ sở sử dụng</dt>
          <dd>{info.facilityName}</dd>
        </div>
        <div>
          <dt>Khách hàng</dt>
          <dd>{info.customerName}</dd>
        </div>
        <div>
          <dt>Hạn sử dụng</dt>
          <dd>{formatExpiry(info.expiresAt)}</dd>
        </div>
      </dl>
    </div>
  );
}
