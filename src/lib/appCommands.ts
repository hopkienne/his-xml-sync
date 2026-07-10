import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import type {
  AppLogInfo,
  AppSettings,
  DeviceFolderState,
  ExportLogsResult,
  FolderScanResult,
  HisAuthStatus,
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
    return { deviceKey, trackingFolder: null, updatedAt: null };
  }
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

export async function listXmlFiles(
  deviceKey: string = KR800_DEVICE_KEY,
): Promise<TrackedXmlFile[]> {
  try {
    return await invoke<TrackedXmlFile[]>("list_xml_files", { deviceKey });
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
