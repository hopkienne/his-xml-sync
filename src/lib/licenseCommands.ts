import { invoke } from "@tauri-apps/api/core";
import type {
  LicenseError,
  LicenseErrorCode,
  LicenseInfo,
  LicenseStatus,
} from "../types/license";

const KNOWN_ERROR_CODES: LicenseErrorCode[] = [
  "INVALID_FORMAT",
  "INVALID_SIGNATURE",
  "EXPIRED",
  "MACHINE_MISMATCH",
  "RUNTIME_UNAVAILABLE",
  "UNKNOWN",
];

function ensureTauriRuntime() {
  if (!("__TAURI_INTERNALS__" in window)) {
    throw { code: "RUNTIME_UNAVAILABLE" } satisfies LicenseError;
  }
}

function normalizeLicenseError(raw: unknown): LicenseError {
  if (typeof raw === "object" && raw !== null && "code" in raw) {
    const candidate = raw as Partial<LicenseError>;
    if (
      typeof candidate.code === "string" &&
      KNOWN_ERROR_CODES.includes(candidate.code as LicenseErrorCode)
    ) {
      return { code: candidate.code as LicenseErrorCode, message: candidate.message };
    }
  }

  if (typeof raw === "string") {
    try {
      return normalizeLicenseError(JSON.parse(raw));
    } catch {
      const matched = KNOWN_ERROR_CODES.find((code) => code === raw);
      return matched ? { code: matched } : { code: "UNKNOWN", message: raw };
    }
  }

  return { code: "UNKNOWN" };
}

export async function activateLicense(key: string): Promise<LicenseInfo> {
  try {
    ensureTauriRuntime();
    return await invoke<LicenseInfo>("activate_license", { key });
  } catch (error) {
    throw normalizeLicenseError(error);
  }
}

export async function getLicenseStatus(): Promise<LicenseStatus> {
  try {
    return await invoke<LicenseStatus>("get_license_status");
  } catch {
    return { valid: false };
  }
}
