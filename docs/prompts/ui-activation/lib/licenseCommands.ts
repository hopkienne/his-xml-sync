import { invoke } from '@tauri-apps/api/core';
import type {
  LicenseError,
  LicenseErrorCode,
  LicenseInfo,
  LicenseStatus,
} from '../types/license';

const KNOWN_ERROR_CODES: LicenseErrorCode[] = [
  'INVALID_FORMAT',
  'INVALID_SIGNATURE',
  'EXPIRED',
  'MACHINE_MISMATCH',
  'UNKNOWN',
];

/**
 * Chuẩn hoá lỗi trả về từ Tauri command thành LicenseError.
 *
 * Command `activate_license` (Rust) nên reject bằng một trong hai dạng:
 *   - Chuỗi JSON: '{"code":"EXPIRED","message":"..."}'
 *   - Chuỗi mã lỗi thuần: "EXPIRED"
 *
 * Mọi trường hợp không khớp (mất kết nối, panic, stub chưa cài) sẽ rơi về
 * UNKNOWN để UI luôn có một thông báo hợp lý thay vì crash.
 */
function normalizeLicenseError(raw: unknown): LicenseError {
  if (typeof raw === 'object' && raw !== null && 'code' in raw) {
    const candidate = raw as Partial<LicenseError>;
    if (
      typeof candidate.code === 'string' &&
      KNOWN_ERROR_CODES.includes(candidate.code as LicenseErrorCode)
    ) {
      return { code: candidate.code as LicenseErrorCode, message: candidate.message };
    }
  }

  if (typeof raw === 'string') {
    try {
      return normalizeLicenseError(JSON.parse(raw));
    } catch {
      const matched = KNOWN_ERROR_CODES.find((code) => code === raw);
      return matched ? { code: matched } : { code: 'UNKNOWN', message: raw };
    }
  }

  return { code: 'UNKNOWN' };
}

/** Gửi key lên backend để xác thực và kích hoạt license. */
export async function activateLicense(key: string): Promise<LicenseInfo> {
  try {
    return await invoke<LicenseInfo>('activate_license', { key });
  } catch (error) {
    throw normalizeLicenseError(error);
  }
}

/**
 * Kiểm tra license hiện tại khi ứng dụng khởi động.
 * Nếu command lỗi (ví dụ chưa từng kích hoạt lần nào), coi như chưa có
 * license hợp lệ thay vì để lỗi văng ra màn hình.
 */
export async function getLicenseStatus(): Promise<LicenseStatus> {
  try {
    return await invoke<LicenseStatus>('get_license_status');
  } catch {
    return { valid: false };
  }
}
