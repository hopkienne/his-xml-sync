export type LicenseErrorCode =
  | "INVALID_FORMAT"
  | "INVALID_SIGNATURE"
  | "EXPIRED"
  | "MACHINE_MISMATCH"
  | "RUNTIME_UNAVAILABLE"
  | "UNKNOWN";

export interface LicenseError {
  code: LicenseErrorCode;
  message?: string;
}

export interface LicenseInfo {
  customerName: string;
  facilityName: string;
  expiresAt: string;
}

export interface LicenseStatus {
  valid: boolean;
  info?: LicenseInfo;
}
