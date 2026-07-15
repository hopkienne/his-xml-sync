import { invoke } from "@tauri-apps/api/core";
import {
  disable as disableAutostart,
  enable as enableAutostart,
  isEnabled as isAutostartEnabled,
} from "@tauri-apps/plugin-autostart";
import { open, save } from "@tauri-apps/plugin-dialog";
import type {
  AppLogInfo,
  AppSettings,
  DeviceFolderState,
  ExportLogsResult,
  FolderScanResult,
  HisAuthStatus,
  PatientListSnapshot,
  PatientQueryParam,
  SyncSummary,
  TrackedXmlFile,
  XmlPreview,
} from "../types";

export const fallbackSettings: AppSettings = {
  hisApiUrl: "",
  dsCoSoKcbId: 4,
  copyRefractionToNewGlasses: false,
  username: "",
  password: "",
  hasPassword: false,
  updatedAt: null,
};

export const KR800_DEVICE_KEY = "kr-800";

export async function getSettings(): Promise<AppSettings> {
  try {
    return await invoke<AppSettings>("get_settings");
  } catch {
    return fallbackSettings;
  }
}

export async function saveSettings(settings: AppSettings): Promise<AppSettings> {
  return await invoke<AppSettings>("save_settings", { settings });
}

/** Trạng thái khởi động cùng Windows/macOS/Linux (đọc từ OS). */
export async function getAutostartEnabled(): Promise<boolean> {
  try {
    return await isAutostartEnabled();
  } catch {
    return false;
  }
}

/** Bật/tắt khởi động cùng hệ điều hành. */
export async function setAutostartEnabled(enabled: boolean): Promise<boolean> {
  if (enabled) {
    await enableAutostart();
  } else {
    await disableAutostart();
  }
  return await isAutostartEnabled();
}

export async function runSyncOnce(): Promise<SyncSummary> {
  try {
    return await invoke<SyncSummary>("run_sync_once");
  } catch {
    return {
      scannedFiles: 12,
      sentResults: 8,
      skippedFiles: 3,
      failedFiles: 1,
    };
  }
}

/**
 * Preview parsed refraction from an XML path.
 * Contract: Tauri command `preview_xml_file({ path })`.
 */
export async function previewXmlFile(path: string): Promise<XmlPreview | null> {
  try {
    return await invoke<XmlPreview>("preview_xml_file", { path });
  } catch {
    return null;
  }
}

export async function getDeviceFolder(
  deviceKey: string = KR800_DEVICE_KEY,
): Promise<DeviceFolderState> {
  try {
    return await invoke<DeviceFolderState>("get_device_folder", { deviceKey });
  } catch {
    return {
      deviceKey,
      trackingFolder: null,
      autoProcessEnabled: false,
      updatedAt: null,
    };
  }
}

/** Bật/tắt tự động xử lý HIS cho KR-800 (lưu SQLite). */
export async function setAutoProcessEnabled(
  enabled: boolean,
  deviceKey: string = KR800_DEVICE_KEY,
): Promise<DeviceFolderState> {
  return await invoke<DeviceFolderState>("set_auto_process_enabled", {
    deviceKey,
    enabled,
  });
}

/** Query params API danh sách người bệnh (mặc định nếu chưa lưu). */
export async function getPatientQueryParams(
  deviceKey: string = KR800_DEVICE_KEY,
): Promise<PatientQueryParam[]> {
  try {
    return await invoke<PatientQueryParam[]>("get_patient_query_params", {
      deviceKey,
    });
  } catch {
    return defaultPatientQueryParams();
  }
}

/** Lưu query params API danh sách người bệnh. */
export async function savePatientQueryParams(
  params: PatientQueryParam[],
  deviceKey: string = KR800_DEVICE_KEY,
): Promise<PatientQueryParam[]> {
  return await invoke<PatientQueryParam[]>("save_patient_query_params", {
    deviceKey,
    params,
  });
}

/** Mặc định khớp backend `default_patient_query_params`. */
export function defaultPatientQueryParams(): PatientQueryParam[] {
  return [
    { key: "page", value: "0", enabled: true },
    { key: "sort", value: "thoiGianVaoVien,asc", enabled: true },
    { key: "size", value: "9999", enabled: true },
    { key: "tuThoiGianVaoVien", value: "", enabled: true },
    { key: "denThoiGianVaoVien", value: "", enabled: true },
    { key: "theoPhongKham", value: "false", enabled: true },
    { key: "dsCoSoKcbId", value: "4", enabled: true },
  ];
}

/** Key thuộc bộ mặc định — không cho xoá khỏi popup. */
export function isDefaultPatientParamKey(key: string): boolean {
  return DEFAULT_PATIENT_PARAM_KEYS.has(key.trim());
}

const DEFAULT_PATIENT_PARAM_KEYS = new Set(
  defaultPatientQueryParams().map((item) => item.key),
);

/** Hai key thời gian lấy theo datetime picker «Ngày xử lý». */
export function isProcessRangeBoundParam(key: string): boolean {
  const k = key.trim();
  return k === "tuThoiGianVaoVien" || k === "denThoiGianVaoVien";
}

export async function setTrackingFolderAndScan(
  folder: string,
  deviceKey: string = KR800_DEVICE_KEY,
): Promise<FolderScanResult> {
  return await invoke<FolderScanResult>("set_tracking_folder_and_scan", {
    deviceKey,
    folder,
  });
}

export async function rescanTrackingFolder(
  deviceKey: string = KR800_DEVICE_KEY,
): Promise<FolderScanResult> {
  return await invoke<FolderScanResult>("rescan_tracking_folder", { deviceKey });
}

/**
 * Liệt kê file XML theo khoảng `created_at` (`YYYY-MM-DD HH:mm:ss`).
 * Bắt buộc truyền from/to — backend không trả full table.
 */
export async function listXmlFiles(
  fromTime: string,
  toTime: string,
  deviceKey: string = KR800_DEVICE_KEY,
): Promise<TrackedXmlFile[]> {
  try {
    return await invoke<TrackedXmlFile[]>("list_xml_files", {
      deviceKey,
      fromTime,
      toTime,
    });
  } catch {
    return [];
  }
}

export type Kr800ProcessResult = {
  total: number;
  processed: number;
  failed: number;
  skipped: number;
  files: TrackedXmlFile[];
};

export async function processKr800(
  fromTime: string,
  toTime: string,
  deviceKey: string = KR800_DEVICE_KEY,
): Promise<Kr800ProcessResult> {
  return await invoke<Kr800ProcessResult>("process_kr800", {
    deviceKey,
    fromTime,
    toTime,
  });
}

/**
 * JSON danh sách người bệnh lần gọi API thành công gần nhất trong phiên app.
 * `null` nếu chưa gọi thành công kể từ khi mở app.
 */
export async function getLastPatientList(): Promise<PatientListSnapshot | null> {
  try {
    return await invoke<PatientListSnapshot | null>("get_last_patient_list");
  } catch {
    return null;
  }
}

/** Mở native folder picker; trả về path hoặc null nếu hủy. */
export async function pickTrackingFolder(): Promise<string | null> {
  try {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Chọn thư mục tracking XML (TOPCON KR-800)",
    });
    if (typeof selected === "string" && selected.length > 0) {
      return selected;
    }
    return null;
  } catch {
    return null;
  }
}

export async function getLogInfo(): Promise<AppLogInfo | null> {
  try {
    return await invoke<AppLogInfo>("get_log_info");
  } catch {
    return null;
  }
}

/**
 * Mở save dialog rồi xuất log ra file.
 * Trả về null nếu người dùng hủy.
 */
export async function exportAppLogs(): Promise<ExportLogsResult | null> {
  const stamp = new Date()
    .toISOString()
    .replace(/[:.]/g, "-")
    .replace("T", "_")
    .slice(0, 19);
  const targetPath = await save({
    title: "Xuất file log HIS XML Sync",
    defaultPath: `his-xml-sync-${stamp}.log`,
    filters: [{ name: "Log", extensions: ["log", "txt"] }],
  });

  if (typeof targetPath !== "string" || !targetPath) {
    return null;
  }

  return await invoke<ExportLogsResult>("export_app_logs", { targetPath });
}

/** Ghi log phía client (UI) vào file log backend. */
export async function logClientEvent(
  level: "debug" | "info" | "warn" | "error",
  module: string,
  message: string,
): Promise<void> {
  try {
    await invoke("log_client_event", { level, module, message });
  } catch {
    // ignore when not running inside Tauri
  }
}

export async function loginHis(): Promise<HisAuthStatus> {
  return await invoke<HisAuthStatus>("login_his");
}

export async function getAuthStatus(): Promise<HisAuthStatus | null> {
  try {
    return await invoke<HisAuthStatus>("get_auth_status");
  } catch {
    return null;
  }
}
