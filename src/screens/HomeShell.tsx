import { KeyRound } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { AppSession } from "../App";
import { Sidebar } from "../components/Sidebar";
import {
  Kr800Panel,
  loadStoredProcessRange,
  saveStoredProcessRange,
  toFilterEndDateTime,
  toHisApiDateTime,
  type Kr800ProcessPhase,
} from "../features/kr800/Kr800Panel";
import { HisSettingsPanel } from "../features/settings/HisSettingsPanel";
import {
  exportAppLogs,
  fallbackSettings,
  getAuthStatus,
  getDeviceFolder,
  getLogInfo,
  getSettings,
  listXmlFiles,
  logClientEvent,
  loginHis,
  pickTrackingFolder,
  processKr800,
  rescanTrackingFolder,
  saveSettings,
  setTrackingFolderAndScan,
} from "../lib/appCommands";
import type {
  AppLogInfo,
  AppSettings,
  HisAuthStatus,
  SidebarNavItem,
  SidebarNavKey,
  TrackedXmlFile,
} from "../types";

type HomeShellProps = {
  session: AppSession;
  onLogout: () => void;
};

const sidebarItems: SidebarNavItem[] = [
  {
    key: "kr-800",
    label: "KR-800",
    description: "Máy đo khúc xạ TOPCON KR-800 — theo dõi folder XML và trạng thái file",
    section: "device",
  },
  {
    key: "settings",
    label: "Cấu hình",
    description: "API URL HIS, tài khoản, mật khẩu, đăng nhập HIS và xuất logs",
    section: "system",
  },
];

export function HomeShell({ session, onLogout }: HomeShellProps) {
  const [activeNav, setActiveNav] = useState<SidebarNavKey>("kr-800");
  const [settings, setSettings] = useState<AppSettings>(fallbackSettings);
  const [connectionLabel, setConnectionLabel] = useState("Chưa kiểm tra");
  const [isSaving, setIsSaving] = useState(false);
  const [isTestingConnection, setIsTestingConnection] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const [trackingFolder, setTrackingFolder] = useState<string | null>(null);
  const [xmlFiles, setXmlFiles] = useState<TrackedXmlFile[]>([]);
  const [isLoadingFiles, setIsLoadingFiles] = useState(false);
  const [isScanning, setIsScanning] = useState(false);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [folderStatus, setFolderStatus] = useState<string | null>(null);
  const [processRange, setProcessRange] = useState(loadStoredProcessRange);

  const [logInfo, setLogInfo] = useState<AppLogInfo | null>(null);
  const [isExportingLogs, setIsExportingLogs] = useState(false);
  const [logStatus, setLogStatus] = useState<string | null>(null);
  const [logError, setLogError] = useState<string | null>(null);

  const [hisAuth, setHisAuth] = useState<HisAuthStatus | null>(null);
  const [hisAuthError, setHisAuthError] = useState<string | null>(null);
  const [processPhase, setProcessPhase] = useState<Kr800ProcessPhase>("idle");
  const authOperationInFlight = useRef(false);

  const currentNav = useMemo(
    () => sidebarItems.find((item) => item.key === activeNav) ?? sidebarItems[0],
    [activeNav],
  );

  const isSettingsView = activeNav === "settings";
  const facilityLabel = session.facilityName ?? session.customerName;
  const isProcessing = processPhase === "running";
  const isHisBusy = isProcessing || isTestingConnection || isSaving;

  const refreshLogInfo = useCallback(async () => {
    const info = await getLogInfo();
    setLogInfo(info);
  }, []);

  const loadKr800Data = useCallback(async () => {
    setIsLoadingFiles(true);
    setFolderError(null);
    try {
      const [folderState, files] = await Promise.all([getDeviceFolder(), listXmlFiles()]);
      setTrackingFolder(folderState.trackingFolder ?? null);
      setXmlFiles(files);
    } catch (error) {
      const message = extractErrorMessage(error) || "Không tải được dữ liệu KR-800.";
      setFolderError(message);
      void logClientEvent("error", "kr800", message);
    } finally {
      setIsLoadingFiles(false);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;

    getSettings().then((loadedSettings) => {
      if (!cancelled) {
        setSettings({ ...fallbackSettings, ...loadedSettings });
      }
    });

    // Đồng bộ trạng thái token đã lưu (nếu có) khi vào Home.
    getAuthStatus().then((status) => {
      if (!cancelled && status) {
        setHisAuth(status);
        if (status.loggedIn) {
          setConnectionLabel(
            `Đã có access_token${status.username ? ` (${status.username})` : ""}`,
          );
        }
      }
    });

    void logClientEvent("info", "ui", "HomeShell mounted");

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<TrackedXmlFile>("kr800:file-progress", ({ payload }) => {
      setXmlFiles((current) => {
        const found = current.some((file) => file.id === payload.id);
        if (!found) return [...current, payload];
        return current.map((file) => (file.id === payload.id ? payload : file));
      });
    });

    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  useEffect(() => {
    if (activeNav === "kr-800") {
      void loadKr800Data();
    }
    if (activeNav === "settings") {
      void refreshLogInfo();
    }
  }, [activeNav, loadKr800Data, refreshLogInfo]);

  async function handleSaveSettings() {
    if (authOperationInFlight.current) return;
    authOperationInFlight.current = true;
    setIsSaving(true);
    setSaveError(null);
    try {
      const savedSettings = await saveSettings(settings);
      setSettings(savedSettings);
      setConnectionLabel("Đã lưu cấu hình — đang đăng nhập HIS…");
      void logClientEvent("info", "ui", "save_settings succeeded; calling login_his");

      // Sau khi lưu TK/MK, login lại để lấy access_token mới.
      try {
        const status = await loginHis();
        setHisAuth(status);
        setHisAuthError(null);
        setConnectionLabel(
          status.loggedIn
            ? `Đăng nhập HIS thành công${status.fullName ? `: ${status.fullName}` : ""}`
            : "Lưu xong nhưng chưa có access_token",
        );
      } catch (loginError) {
        const message = extractErrorMessage(loginError) || "Đăng nhập HIS thất bại.";
        setHisAuthError(message);
        setConnectionLabel(message);
      }
    } catch (error) {
      const message = extractErrorMessage(error) || "Không lưu được cấu hình.";
      setSaveError(message);
      setConnectionLabel(message);
      void logClientEvent("error", "ui", `save_settings failed: ${message}`);
    } finally {
      setIsSaving(false);
      authOperationInFlight.current = false;
    }
  }

  async function handleTestConnection() {
    if (authOperationInFlight.current) return;
    authOperationInFlight.current = true;
    setIsTestingConnection(true);
    setSaveError(null);
    try {
      // Nếu form có thay đổi chưa lưu — dùng credentials đã lưu trong SQLite.
      const status = await loginHis();
      setHisAuth(status);
      setHisAuthError(null);
      const label = status.loggedIn
        ? `Kết nối HIS OK — token lưu cho ${status.username ?? "user"}`
        : "Login xong nhưng không có token";
      setConnectionLabel(label);
      void logClientEvent("info", "ui", label);
    } catch (error) {
      const message = extractErrorMessage(error) || "Kiểm tra kết nối thất bại.";
      setHisAuthError(message);
      setConnectionLabel(message);
      void logClientEvent("error", "ui", `test_connection failed: ${message}`);
    } finally {
      setIsTestingConnection(false);
      authOperationInFlight.current = false;
    }
  }

  async function handleExportLogs() {
    setIsExportingLogs(true);
    setLogError(null);
    setLogStatus(null);
    try {
      const result = await exportAppLogs();
      if (!result) {
        setLogStatus("Đã hủy xuất logs.");
        void logClientEvent("info", "ui", "export_app_logs cancelled by user");
        return;
      }
      setLogStatus(
        `Đã xuất ${formatBytes(result.bytesWritten)} từ ${result.sourceFiles} file nguồn → ${result.targetPath}`,
      );
      await refreshLogInfo();
    } catch (error) {
      const message = extractErrorMessage(error) || "Không xuất được logs.";
      setLogError(message);
      void logClientEvent("error", "ui", `export_app_logs failed: ${message}`);
    } finally {
      setIsExportingLogs(false);
    }
  }

  async function handlePickFolder() {
    setFolderError(null);
    setFolderStatus(null);
    const folder = await pickTrackingFolder();
    if (!folder) {
      setFolderStatus("Đã hủy chọn thư mục.");
      void logClientEvent("info", "ui", "pick_tracking_folder cancelled");
      return;
    }

    setIsScanning(true);
    try {
      const result = await setTrackingFolderAndScan(folder);
      setTrackingFolder(result.trackingFolder);
      setXmlFiles(result.files);
      setFolderStatus(
        `Đã quét ${result.scannedCount} file XML, thêm mới ${result.insertedCount} bản ghi.`,
      );
    } catch (error) {
      const message = extractErrorMessage(error) || "Không quét được thư mục.";
      setFolderError(message);
      void logClientEvent("error", "ui", `set_tracking_folder_and_scan failed: ${message}`);
    } finally {
      setIsScanning(false);
    }
  }

  async function handleRescan() {
    setFolderError(null);
    setFolderStatus(null);
    setIsScanning(true);
    try {
      const result = await rescanTrackingFolder();
      setTrackingFolder(result.trackingFolder);
      setXmlFiles(result.files);
      setFolderStatus(
        `Quét lại: ${result.scannedCount} file XML, thêm mới ${result.insertedCount} bản ghi.`,
      );
    } catch (error) {
      const message = extractErrorMessage(error) || "Không quét lại được thư mục.";
      setFolderError(message);
      void logClientEvent("error", "ui", `rescan_tracking_folder failed: ${message}`);
    } finally {
      setIsScanning(false);
    }
  }

  async function handleProcess() {
    if (authOperationInFlight.current) return;
    authOperationInFlight.current = true;

    setProcessPhase("running");
    setHisAuthError(null);
    setFolderStatus("Đang tải danh sách người bệnh và xử lý tối đa 5 file cùng lúc…");
    setFolderError(null);
    void logClientEvent("info", "kr800", "process pipeline started");

    try {
      const result = await processKr800(
        toHisApiDateTime(processRange.from),
        toFilterEndDateTime(processRange.to),
      );
      setXmlFiles(result.files);
      setHisAuthError(null);
      setProcessPhase("success");
      setFolderStatus(
        result.total === 0
          ? "Không có file ở trạng thái Chờ xử lý trong khoảng thời gian đã chọn."
          : `Đã xử lý ${result.processed}/${result.total} file; bỏ qua trùng ${result.skipped}; lỗi ${result.failed}.`,
      );
      const status = await getAuthStatus();
      setHisAuth(status);
      setConnectionLabel(status?.hasAccessToken ? "HIS: đã có access_token" : "HIS: chưa login");
      void logClientEvent("info", "kr800", "process pipeline completed");
    } catch (error) {
      const message = extractErrorMessage(error) || "Không thực hiện được luồng xử lý KR-800.";
      setFolderError(message);
      setProcessPhase("error");
      void logClientEvent("error", "kr800", `process pipeline failed: ${message}`);
    } finally {
      authOperationInFlight.current = false;
    }
  }

  function handleProcessRangeChange(next: typeof processRange) {
    saveStoredProcessRange(next);
    setProcessRange(next);
  }

  return (
    <main className="app-shell">
      <Sidebar
        items={sidebarItems}
        activeKey={activeNav}
        onSelect={setActiveNav}
        facilityLabel={facilityLabel}
      />

      <section className="content-area">
        <header className="topbar">
          <div className="topbar__titles">
            <h1>{currentNav.label}</h1>
            <p>{currentNav.description}</p>
          </div>
          <div className="session-pill">
            <span
              className={`session-pill__dot${
                isHisBusy ? " is-busy" : hisAuth?.loggedIn ? "" : " is-warn"
              }`}
              aria-hidden="true"
            />
            <div className="session-pill__meta">
              <span title={facilityLabel}>{facilityLabel ?? "Chưa gán tên"}</span>
              <strong>
                {isHisBusy
                  ? "Đang đăng nhập HIS…"
                  : hisAuth?.loggedIn
                    ? `HIS: ${hisAuth.fullName || hisAuth.username || "đã login"}`
                    : hisAuthError
                      ? "HIS: lỗi login"
                      : "HIS: chưa login"}
              </strong>
            </div>
            <button
              type="button"
              className="ds-button ds-button--ghost"
              onClick={onLogout}
              disabled={isHisBusy}
            >
              <KeyRound size={14} strokeWidth={2} aria-hidden="true" />
              Đổi key
            </button>
          </div>
        </header>

        <section className="work-surface">
          {isSettingsView ? (
            <HisSettingsPanel
              settings={settings}
              onChange={(next) => {
                setSaveError(null);
                setSettings(next);
              }}
              onSave={handleSaveSettings}
              onTestConnection={handleTestConnection}
              onExportLogs={handleExportLogs}
              connectionLabel={connectionLabel}
              isSaving={isSaving}
              isTestingConnection={isTestingConnection}
              isProcessing={isProcessing}
              isExportingLogs={isExportingLogs}
              saveError={saveError}
              logInfo={logInfo}
              logStatus={logStatus}
              logError={logError}
              hisAuth={hisAuth}
              hisAuthError={hisAuthError}
            />
          ) : (
            <Kr800Panel
              trackingFolder={trackingFolder}
              files={xmlFiles}
              isLoading={isLoadingFiles}
              isScanning={isScanning}
              error={folderError}
              statusMessage={folderStatus}
              onPickFolder={handlePickFolder}
              onRescan={handleRescan}
              processRange={processRange}
              onProcessRangeChange={handleProcessRangeChange}
              processPhase={processPhase}
              isProcessBlocked={isSaving || isTestingConnection}
              onProcess={() => void handleProcess()}
            />
          )}
        </section>
      </section>
    </main>
  );
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

function extractErrorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") return message;
  }
  return "";
}
