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
import { Hdr9000Panel } from "../features/hdr9000/Hdr9000Panel";
import { Ct800Panel } from "../features/ct800/Ct800Panel";
import { HisSettingsPanel } from "../features/settings/HisSettingsPanel";
import {
  exportAppLogs,
  fallbackSettings,
  getAuthStatus,
  getAutostartEnabled,
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
  setAutoProcessEnabled,
  setAutostartEnabled,
  setTrackingFolderAndScan,
} from "../lib/appCommands";
import type {
  AppLogInfo,
  AppSettings,
  HisAuthStatus,
  Kr800AutoProcessEvent,
  Kr800FilesIndexedEvent,
  Kr800ScanProgressEvent,
  Kr800WatchStatusEvent,
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
    key: "hdr-9000",
    label: "HDR-9000",
    description: "Theo dõi XML HDR-9000, revision và trạng thái gửi HIS",
    section: "device",
  },
  {
    key: "ct-800",
    label: "CT-800",
    description: "Theo dõi XML nhãn áp TOPCON CT-800 và trạng thái gửi HIS",
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
  const [scanProgress, setScanProgress] = useState<Kr800ScanProgressEvent | null>(null);
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
  const [autoWatchActive, setAutoWatchActive] = useState(false);
  const [autoWatchMessage, setAutoWatchMessage] = useState<string | null>(null);
  const [autoProcessEnabled, setAutoProcessEnabledState] = useState(false);
  const [isTogglingAutoProcess, setIsTogglingAutoProcess] = useState(false);
  const [autoProcessError, setAutoProcessError] = useState<string | null>(null);
  const [autoProcessStatus, setAutoProcessStatus] = useState<string | null>(null);
  const [autostartEnabled, setAutostartEnabledState] = useState(false);
  const [isTogglingAutostart, setIsTogglingAutostart] = useState(false);
  const [autostartError, setAutostartError] = useState<string | null>(null);
  const [autostartStatus, setAutostartStatus] = useState<string | null>(null);
  const authOperationInFlight = useRef(false);
  /** Chống spam Quét lại / Chọn thư mục khi command còn chạy. */
  const scanInFlight = useRef(false);
  /** Tránh refresh list chồng khi event nền dồn. */
  const refreshInFlight = useRef(false);
  const processRangeRef = useRef(processRange);
  processRangeRef.current = processRange;

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

  /** Chỉ load file theo khoảng created_at (processRange) — không load full 15k bản ghi. */
  const loadKr800Data = useCallback(async () => {
    setIsLoadingFiles(true);
    setFolderError(null);
    try {
      const fromTime = toHisApiDateTime(processRange.from);
      const toTime = toFilterEndDateTime(processRange.to);
      const [folderState, files] = await Promise.all([
        getDeviceFolder(),
        fromTime && toTime ? listXmlFiles(fromTime, toTime) : Promise.resolve([] as TrackedXmlFile[]),
      ]);
      setTrackingFolder(folderState.trackingFolder ?? null);
      setAutoProcessEnabledState(Boolean(folderState.autoProcessEnabled));
      setXmlFiles(files);
    } catch (error) {
      const message = extractErrorMessage(error) || "Không tải được dữ liệu KR-800.";
      setFolderError(message);
      void logClientEvent("error", "kr800", message);
    } finally {
      setIsLoadingFiles(false);
    }
  }, [processRange.from, processRange.to]);

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
        const created = normalizeCreatedAt(payload.createdAt);
        const range = processRangeRef.current;
        const from = toHisApiDateTime(range.from);
        const to = toFilterEndDateTime(range.to);
        const inRange = Boolean(created && from && to && created >= from && created <= to);

        const found = current.some((file) => file.id === payload.id);
        if (!found) {
          return inRange ? [...current, payload] : current;
        }
        // File ra ngoài range (hiếm) → gỡ khỏi bảng đang xem.
        if (!inRange) {
          return current.filter((file) => file.id !== payload.id);
        }
        return current.map((file) => (file.id === payload.id ? payload : file));
      });
    });

    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  /** Background: file mới index / auto-process / watch status. */
  useEffect(() => {
    const unsubs: Array<Promise<() => void>> = [];

    unsubs.push(
      listen<Kr800FilesIndexedEvent>("kr800:files-indexed", ({ payload }) => {
        setAutoWatchActive(true);
        setFolderStatus(
          `Tự phát hiện ${payload.insertedCount} file XML mới (${payload.source}). Đang làm mới danh sách…`,
        );
        void quietRefreshFiles();
      }),
    );

    unsubs.push(
      listen<Kr800AutoProcessEvent>("kr800:auto-process", ({ payload }) => {
        if (payload.busy) {
          setFolderStatus(payload.message);
          return;
        }
        if (!payload.ok) {
          setFolderStatus(payload.message);
          // Vẫn refresh để thấy file waiting mới index.
          void quietRefreshFiles();
          return;
        }
        if (payload.total > 0 || payload.message) {
          setFolderStatus(payload.message);
        }
        if (payload.total > 0) {
          // awaiting_pair (chờ lần đo 2) không coi là lỗi pipeline.
          const onlyAwaiting =
            payload.failed === 0 &&
            payload.processed === 0 &&
            /chờ lần đo 2/i.test(payload.message);
          setProcessPhase(
            onlyAwaiting
              ? "success"
              : payload.failed > 0 && payload.processed === 0
                ? "error"
                : "success",
          );
        }
        void quietRefreshFiles();
        void getAuthStatus().then((status) => {
          if (status) {
            setHisAuth(status);
            setConnectionLabel(
              status.hasAccessToken ? "HIS: đã có access_token" : "HIS: chưa login",
            );
          }
        });
      }),
    );

    unsubs.push(
      listen<Kr800WatchStatusEvent>("kr800:watch-status", ({ payload }) => {
        setAutoWatchActive(payload.active);
        setAutoWatchMessage(payload.message);
        if (payload.trackingFolder) {
          setTrackingFolder(payload.trackingFolder);
        }
      }),
    );

    unsubs.push(
      listen<Kr800ScanProgressEvent>("kr800:scan-progress", ({ payload }) => {
        setScanProgress(payload);
      }),
    );

    return () => {
      for (const p of unsubs) {
        void p.then((dispose) => dispose());
      }
    };
    // quietRefreshFiles ổn định qua ref range; không cần deps loadKr800Data.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function quietRefreshFiles() {
    if (refreshInFlight.current) return;
    refreshInFlight.current = true;
    try {
      const range = processRangeRef.current;
      const fromTime = toHisApiDateTime(range.from);
      const toTime = toFilterEndDateTime(range.to);
      if (!fromTime || !toTime) return;
      const files = await listXmlFiles(fromTime, toTime);
      setXmlFiles(files);
    } catch {
      // ignore background refresh errors
    } finally {
      refreshInFlight.current = false;
    }
  }

  function normalizeCreatedAt(value?: string | null): string {
    if (!value) return "";
    return toHisApiDateTime(value.trim());
  }

  useEffect(() => {
    if (activeNav === "kr-800") {
      void loadKr800Data();
    }
    if (activeNav === "settings") {
      void refreshLogInfo();
      // Đồng bộ cờ auto-process + folder khi vào Cấu hình.
      void getDeviceFolder().then((state) => {
        setAutoProcessEnabledState(Boolean(state.autoProcessEnabled));
        setTrackingFolder(state.trackingFolder ?? null);
      });
      void getAutostartEnabled().then((enabled) => {
        setAutostartEnabledState(enabled);
      });
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

  function formatScanStatus(prefix: string, result: Awaited<ReturnType<typeof rescanTrackingFolder>>) {
    const parts = [
      `${prefix}: quét ${result.scannedCount} file XML`,
      `thêm mới ${result.insertedCount}`,
      `cập nhật ${result.updatedCount}`,
    ];
    if (result.prunedCount > 0) {
      parts.push(`gỡ ${result.prunedCount} bản ghi thiếu file`);
    }
    parts.push(`tổng theo dõi ${result.trackedCount}`);
    let message = `${parts.join(", ")}.`;
    if (result.pruneSkipped) {
      message +=
        " Đã bỏ qua xóa hàng loạt vì số file trên disk giảm đột biến (bảo vệ dữ liệu).";
    }
    return message;
  }

  async function handlePickFolder() {
    if (scanInFlight.current) return;
    setFolderError(null);
    setFolderStatus(null);
    const folder = await pickTrackingFolder();
    if (!folder) {
      setFolderStatus("Đã hủy chọn thư mục.");
      void logClientEvent("info", "ui", "pick_tracking_folder cancelled");
      return;
    }

    scanInFlight.current = true;
    setIsScanning(true);
    setScanProgress({
      phase: "disk",
      current: 0,
      total: 0,
      percent: 0,
      message: "Bắt đầu quét thư mục…",
    });
    try {
      const result = await setTrackingFolderAndScan(folder);
      setTrackingFolder(result.trackingFolder);
      const autoHint = autoProcessEnabled
        ? " Tự động xử lý đang BẬT — file waiting sẽ được gửi HIS."
        : "";
      setFolderStatus(formatScanStatus("Đã quét thư mục", result) + autoHint);
      setFolderError(null);
      // Reload theo khoảng ngày hiện tại — không nhận full list từ scan.
      await loadKr800Data();
    } catch (error) {
      const message = extractErrorMessage(error) || "Không quét được thư mục.";
      setFolderError(message);
      void logClientEvent("error", "ui", `set_tracking_folder_and_scan failed: ${message}`);
    } finally {
      setIsScanning(false);
      setScanProgress(null);
      scanInFlight.current = false;
    }
  }

  async function handleRescan() {
    if (scanInFlight.current) return;
    setFolderError(null);
    setFolderStatus(null);
    scanInFlight.current = true;
    setIsScanning(true);
    setScanProgress({
      phase: "disk",
      current: 0,
      total: 0,
      percent: 0,
      message: "Bắt đầu quét lại…",
    });
    try {
      const result = await rescanTrackingFolder();
      setTrackingFolder(result.trackingFolder);
      setFolderStatus(formatScanStatus("Quét lại", result));
      await loadKr800Data();
    } catch (error) {
      const message = extractErrorMessage(error) || "Không quét lại được thư mục.";
      setFolderError(message);
      void logClientEvent("error", "ui", `rescan_tracking_folder failed: ${message}`);
    } finally {
      setIsScanning(false);
      setScanProgress(null);
      scanInFlight.current = false;
    }
  }

  async function handleProcess() {
    if (authOperationInFlight.current) return;
    authOperationInFlight.current = true;

    setProcessPhase("running");
    setHisAuthError(null);
    setFolderStatus("Đang tải danh sách người bệnh và ghép cặp / gửi HIS…");
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
      const awaiting = result.awaitingPair ?? 0;
      setFolderStatus(
        result.total === 0
          ? "Không có file ở trạng thái Chờ xử lý trong khoảng thời gian đã chọn."
          : awaiting > 0 && result.processed === 0 && result.failed === 0
            ? `Đã nhận ${awaiting} lần đo 1, đang chờ lần đo 2.`
            : `Đã xử lý ${result.processed}/${result.total}; chờ lần đo 2: ${awaiting}; bỏ qua ${result.skipped}; lỗi ${result.failed}.`,
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

  async function handleAutoProcessChange(enabled: boolean) {
    setIsTogglingAutoProcess(true);
    setAutoProcessError(null);
    setAutoProcessStatus(null);
    try {
      const state = await setAutoProcessEnabled(enabled);
      setAutoProcessEnabledState(Boolean(state.autoProcessEnabled));
      setTrackingFolder(state.trackingFolder ?? null);

      if (enabled && !(state.trackingFolder && state.trackingFolder.trim())) {
        setAutoProcessError(
          "Chưa chọn thư mục tracking. Vào tab KR-800 → Chọn thư mục để tự động xử lý tiếp tục.",
        );
        setAutoProcessStatus("Tự động xử lý đã BẬT — đang chờ thư mục tracking.");
      } else if (enabled) {
        setAutoProcessStatus(
          "Tự động xử lý đã BẬT — app sẽ tự gửi file waiting lên HIS khi có file mới.",
        );
      } else {
        setAutoProcessStatus("Tự động xử lý đã TẮT — dùng nút «Xử lý» trên tab KR-800 khi cần.");
      }
      void logClientEvent(
        "info",
        "kr800",
        `auto_process_enabled=${enabled} folder=${state.trackingFolder ?? ""}`,
      );
    } catch (error) {
      const message =
        extractErrorMessage(error) || "Không lưu được cấu hình tự động xử lý.";
      setAutoProcessError(message);
      void logClientEvent("error", "kr800", `set_auto_process_enabled failed: ${message}`);
    } finally {
      setIsTogglingAutoProcess(false);
    }
  }

  async function handleAutostartChange(enabled: boolean) {
    setIsTogglingAutostart(true);
    setAutostartError(null);
    setAutostartStatus(null);
    try {
      const next = await setAutostartEnabled(enabled);
      setAutostartEnabledState(next);
      setAutostartStatus(
        next
          ? "Đã bật khởi động cùng Windows — app sẽ tự chạy sau khi đăng nhập."
          : "Đã tắt khởi động cùng Windows.",
      );
      void logClientEvent("info", "ui", `autostart_enabled=${next}`);
    } catch (error) {
      const message =
        extractErrorMessage(error) || "Không cập nhật được cấu hình khởi động cùng Windows.";
      setAutostartError(message);
      // Đồng bộ lại trạng thái thật từ OS nếu thao tác lỗi giữa chừng.
      void getAutostartEnabled().then(setAutostartEnabledState);
      void logClientEvent("error", "ui", `set_autostart failed: ${message}`);
    } finally {
      setIsTogglingAutostart(false);
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
              autoProcessEnabled={autoProcessEnabled}
              isTogglingAutoProcess={isTogglingAutoProcess}
              trackingFolder={trackingFolder}
              autoProcessError={autoProcessError}
              autoProcessStatus={autoProcessStatus}
              onAutoProcessChange={(enabled) => void handleAutoProcessChange(enabled)}
              autostartEnabled={autostartEnabled}
              isTogglingAutostart={isTogglingAutostart}
              autostartError={autostartError}
              autostartStatus={autostartStatus}
              onAutostartChange={(enabled) => void handleAutostartChange(enabled)}
            />
          ) : activeNav === "hdr-9000" ? (
            <Hdr9000Panel />
          ) : activeNav === "ct-800" ? (
            <Ct800Panel />
          ) : (
            <Kr800Panel
              trackingFolder={trackingFolder}
              files={xmlFiles}
              isLoading={isLoadingFiles}
              isScanning={isScanning}
              scanProgress={scanProgress}
              error={folderError}
              statusMessage={folderStatus}
              autoWatchActive={autoWatchActive}
              autoWatchMessage={autoWatchMessage}
              autoProcessEnabled={autoProcessEnabled}
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
