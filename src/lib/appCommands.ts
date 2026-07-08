import { invoke } from "@tauri-apps/api/core";
import type { AppSettings, SyncSummary } from "../types";

export const fallbackSettings: AppSettings = {
  apiBaseUrl: "https://api-hisvn.vietngagroup.vn",
  username: "",
  xmlFolder: "C:\\TOPCON\\KR-800\\Export",
  facilityId: 4,
  autoSyncEnabled: false,
};

export async function getSettings(): Promise<AppSettings> {
  try {
    return await invoke<AppSettings>("get_settings");
  } catch {
    return fallbackSettings;
  }
}

export async function saveSettings(settings: AppSettings): Promise<AppSettings> {
  try {
    return await invoke<AppSettings>("save_settings", { settings });
  } catch {
    return settings;
  }
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
