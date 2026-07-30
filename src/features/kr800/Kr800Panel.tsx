import { listen } from "@tauri-apps/api/event";
import {
  ChevronDown,
  ChevronRight,
  FolderOpen,
  Loader2,
  Play,
  RefreshCw,
  SlidersHorizontal,
  Users,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { getLastPatientList } from "../../lib/appCommands";
import type {
  Kr800ScanProgressEvent,
  PatientListReadyEvent,
  TrackedXmlFile,
  TrackedXmlStatus,
} from "../../types";
import { PatientListDialog } from "./PatientListDialog";
import { PatientParamsDialog } from "./PatientParamsDialog";

const FOLDER_COLLAPSE_KEY = "his-xml-sync.kr800-folder-collapsed";
const PROCESS_RANGE_KEY = "his-xml-sync.kr800-process-range";

export type ProcessDateRange = {
  /** Giá trị input datetime-local: YYYY-MM-DDTHH:mm */
  from: string;
  to: string;
};

export type Kr800ProcessPhase = "idle" | "running" | "success" | "error";

type Kr800PanelProps = {
  trackingFolder: string | null;
  files: TrackedXmlFile[];
  isLoading: boolean;
  isScanning: boolean;
  /** Tiến trình quét thật từ backend (chọn folder / Quét lại). */
  scanProgress?: Kr800ScanProgressEvent | null;
  error?: string | null;
  statusMessage?: string | null;
  /** Đang bật quét nền folder (FS watcher / poll). */
  autoWatchActive?: boolean;
  autoWatchMessage?: string | null;
  /** Cờ user (cấu hình ở trang Cấu hình): tự xử lý HIS khi có file waiting. */
  autoProcessEnabled?: boolean;
  onPickFolder: () => void;
  onRescan: () => void;
  /** Khoảng thời gian xử lý (từ–đến, có giờ). */
  processRange?: ProcessDateRange;
  onProcessRangeChange?: (range: ProcessDateRange) => void;
  processPhase: Kr800ProcessPhase;
  isProcessBlocked?: boolean;
  onProcess: () => void;
};

const statusLabel: Record<TrackedXmlStatus, string> = {
  waiting: "Chờ xử lý",
  processing: "Đang xử lý",
  parsed: "Đã phân tích XML",
  patient_matched: "Đã tìm thấy bệnh nhân",
  mapped: "Đã mapping danh mục",
  sending: "Đang gửi HIS",
  processed: "Đã xử lý",
  patient_not_found: "Không tìm thấy bệnh nhân",
  treatment_ambiguous: "Không xác định đợt điều trị",
  service_not_found: "Không tìm thấy dịch vụ khám",
  xml_error: "Lỗi XML",
  mapping_error: "Lỗi mapping danh mục",
  send_error: "Lỗi gửi HIS",
  failed: "Thất bại",
  awaiting_pair: "Đã gửi lần đo 1, chờ lần đo 2",
  pairing: "Đang ghép cặp",
  pairing_error: "Lỗi ghép cặp",
  extra_measurement: "Phát hiện lần đo thừa",
};

export function Kr800Panel({
  trackingFolder,
  files,
  isLoading,
  isScanning,
  scanProgress = null,
  error,
  statusMessage,
  autoWatchActive = false,
  autoWatchMessage = null,
  autoProcessEnabled = false,
  onPickFolder,
  onRescan,
  processRange: controlledRange,
  onProcessRangeChange,
  processPhase,
  isProcessBlocked = false,
  onProcess,
}: Kr800PanelProps) {
  const [localRange, setLocalRange] = useState<ProcessDateRange>(loadStoredProcessRange);
  const processRange = controlledRange ?? localRange;
  const [paramsOpen, setParamsOpen] = useState(false);
  const [patientsOpen, setPatientsOpen] = useState(false);
  /** Bật sau khi API danh sách người bệnh gọi thành công trong phiên app. */
  const [patientListReady, setPatientListReady] = useState(false);
  const [patientListMeta, setPatientListMeta] = useState<PatientListReadyEvent | null>(null);
  // Keep the dialog prop stable: this panel receives frequent file-progress
  // events while an auto-processing run is active.
  const closePatientList = useCallback(() => setPatientsOpen(false), []);

  useEffect(() => {
    let cancelled = false;
    void getLastPatientList().then((data) => {
      if (cancelled || !data) return;
      setPatientListReady(true);
      setPatientListMeta({
        patientCount: data.patientCount,
        fromTime: data.fromTime,
        toTime: data.toTime,
        fetchedAt: data.fetchedAt,
      });
    });
    const unlisten = listen<PatientListReadyEvent>("kr800:patient-list-ready", ({ payload }) => {
      setPatientListReady(true);
      setPatientListMeta(payload);
    });
    return () => {
      cancelled = true;
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  const [folderCollapsed, setFolderCollapsed] = useState(() => {
    try {
      // Mặc định thu gọn nếu đã có folder (đọc sau mount cũng sync).
      return window.localStorage.getItem(FOLDER_COLLAPSE_KEY) === "1";
    } catch {
      return false;
    }
  });

  // Khi chọn folder xong lần đầu → auto thu gọn section tracking.
  useEffect(() => {
    if (!trackingFolder) {
      setFolderCollapsed(false);
      return;
    }
    try {
      const stored = window.localStorage.getItem(FOLDER_COLLAPSE_KEY);
      if (stored === null) {
        setFolderCollapsed(true);
        window.localStorage.setItem(FOLDER_COLLAPSE_KEY, "1");
      }
    } catch {
      setFolderCollapsed(true);
    }
  }, [trackingFolder]);

  const rangeError = useMemo(() => {
    if (!processRange.from || !processRange.to) return null;
    if (processRange.from > processRange.to) {
      return "Thời gian “Từ” phải nhỏ hơn hoặc bằng “Đến”.";
    }
    return null;
  }, [processRange.from, processRange.to]);

  /**
   * Backend đã lọc theo `created_at` + khoảng ngày.
   * Giữ filter client nhẹ như lớp an toàn nếu state còn sót file ngoài range.
   */
  const filteredFiles = useMemo(() => {
    if (rangeError || !processRange.from || !processRange.to) {
      return [];
    }
    return filterFilesByCreatedAt(files, processRange.from, processRange.to);
  }, [files, processRange.from, processRange.to, rangeError]);

  const counts = countByStatus(filteredFiles);
  const processDisabledReason =
    processPhase === "running"
      ? "Luồng xử lý đang chạy."
      : isProcessBlocked
        ? "Đang hoàn tất một thao tác kết nối HIS khác."
        : isLoading
          ? "Danh sách file đang được tải."
          : isScanning
            ? "Thư mục đang được quét."
            : !trackingFolder
              ? "Chọn thư mục tracking trước khi xử lý."
              : rangeError
                ? "Chỉnh lại khoảng thời gian trước khi xử lý."
                : null;
  const processButtonLabel =
    processPhase === "running"
      ? "Đang xử lý…"
      : "Xử lý";

  function setProcessRange(next: ProcessDateRange) {
    if (onProcessRangeChange) {
      onProcessRangeChange(next);
    } else {
      saveStoredProcessRange(next);
      setLocalRange(next);
    }
  }

  function toggleFolderCollapsed() {
    setFolderCollapsed((prev) => {
      const next = !prev;
      try {
        window.localStorage.setItem(FOLDER_COLLAPSE_KEY, next ? "1" : "0");
      } catch {
        /* ignore */
      }
      return next;
    });
  }

  return (
    <section className="kr800-panel panel-stack">
      <div className={`kr800-folder ds-card${folderCollapsed ? " is-collapsed" : ""}`}>
        <div className="panel-heading kr800-folder__heading">
          <button
            type="button"
            className="kr800-collapse-toggle"
            onClick={toggleFolderCollapsed}
            aria-expanded={!folderCollapsed}
            title={folderCollapsed ? "Mở rộng thư mục tracking" : "Thu gọn thư mục tracking"}
          >
            {folderCollapsed ? (
              <ChevronRight size={18} strokeWidth={2} aria-hidden="true" />
            ) : (
              <ChevronDown size={18} strokeWidth={2} aria-hidden="true" />
            )}
            <span className="kr800-collapse-toggle__titles">
              <span className="kr800-collapse-toggle__title">Thư mục tracking</span>
              {folderCollapsed ? (
                <span className="kr800-collapse-toggle__summary" title={trackingFolder || undefined}>
                  {trackingFolder || "Chưa chọn thư mục"}
                </span>
              ) : null}
            </span>
          </button>

          <div className="panel-actions">
            <button
              type="button"
              className="ds-button ds-button--ghost"
              onClick={onRescan}
              disabled={!trackingFolder || isScanning || isLoading}
            >
              <RefreshCw
                size={16}
                strokeWidth={2}
                className={isScanning ? "spin" : undefined}
                aria-hidden="true"
              />
              Quét lại
            </button>
            <button
              type="button"
              className="ds-button ds-button--primary"
              onClick={onPickFolder}
              disabled={isScanning || isLoading}
            >
              <FolderOpen size={16} strokeWidth={2} aria-hidden="true" />
              Chọn thư mục
            </button>
          </div>
        </div>

        {!folderCollapsed ? (
          <div className="kr800-folder__body">
            <p className="panel-lead">
              Chọn thư mục lưu file XML. Ứng dụng quét file <code>.xml</code> để xử lý.
            </p>

            <div className="path-box">
              <span>Đường dẫn đang theo dõi</span>
              <strong title={trackingFolder || undefined}>
                {trackingFolder || "Chưa chọn thư mục"}
              </strong>
            </div>

            {trackingFolder ? (
              <p
                className={`kr800-message${autoWatchActive ? " kr800-message--watch-on" : ""}`}
                title={autoWatchMessage || undefined}
              >
                {autoWatchActive
                  ? autoProcessEnabled
                    ? "● Theo dõi folder: BẬT — quét nền + tự xử lý HIS"
                    : "● Theo dõi folder: BẬT — chỉ quét nền (tự xử lý đang TẮT)"
                  : "○ Theo dõi folder: đang kết nối… (sẽ bật sau khi app nhận folder)"}
              </p>
            ) : null}

            <div className="kr800-status-row" aria-live="polite">
              <span className="sync-summary-chip" title="Số file theo ngày tạo (created_at) trong khoảng đã chọn">
                Trong khoảng <strong>{filteredFiles.length}</strong>
              </span>
              <span className="sync-summary-chip">
                Chờ <strong>{counts.waiting}</strong>
              </span>
              <span className="sync-summary-chip">
                Đang xử lý <strong>{counts.processing}</strong>
              </span>
              <span className="sync-summary-chip">
                Đã xử lý <strong>{counts.processed}</strong>
              </span>
              <span className="sync-summary-chip">
                Thất bại <strong>{counts.failed}</strong>
              </span>
            </div>

            {isScanning ? <ScanProgressBar progress={scanProgress} /> : null}
            {statusMessage ? <p className="kr800-message">{statusMessage}</p> : null}
            {error ? (
              <p className="settings-error" role="alert">
                {error}
              </p>
            ) : null}
          </div>
        ) : (
          <div className="kr800-folder__collapsed-meta">
            {isScanning ? <ScanProgressBar progress={scanProgress} compact /> : null}
            <div className="kr800-status-row" aria-live="polite">
              <span className="sync-summary-chip" title="Số file theo ngày tạo trong khoảng đã chọn">
                Trong khoảng <strong>{filteredFiles.length}</strong>
              </span>
              <span className="sync-summary-chip">
                Chờ <strong>{counts.waiting}</strong>
              </span>
              <span className="sync-summary-chip">
                Lỗi <strong>{counts.failed}</strong>
              </span>
            </div>
            {statusMessage ? <p className="kr800-message">{statusMessage}</p> : null}
            {error ? (
              <p className="settings-error" role="alert">
                {error}
              </p>
            ) : null}
          </div>
        )}
      </div>

      <div className="kr800-table-block">
        <div className="panel-heading kr800-table-block__heading">
          <h2>Danh sách file XML</h2>

          <div
            className="kr800-range"
            title="Chọn từ–đến (có giờ). Dùng khi đối chiếu người bệnh / lọc phiên theo ngày vào viện."
          >
            <div className="kr800-range__row">
              <span className="kr800-range__label">Ngày xử lý</span>
              <label className="kr800-range__field">
                <span className="visually-hidden">Từ ngày giờ</span>
                <input
                  type="datetime-local"
                  value={processRange.from}
                  max={processRange.to || undefined}
                  aria-label="Từ ngày giờ"
                  title={`Từ: ${formatRangeDisplay(processRange.from)} (API: ${toHisApiDateTime(processRange.from)})`}
                  onChange={(event) =>
                    setProcessRange({ ...processRange, from: event.target.value })
                  }
                />
              </label>
              <span className="kr800-range__sep" aria-hidden="true">
                →
              </span>
              <label className="kr800-range__field">
                <span className="visually-hidden">Đến ngày giờ</span>
                <input
                  type="datetime-local"
                  value={processRange.to}
                  min={processRange.from || undefined}
                  aria-label="Đến ngày giờ"
                  title={`Đến: ${formatRangeDisplay(processRange.to)} (API: ${toHisApiDateTime(processRange.to)})`}
                  onChange={(event) =>
                    setProcessRange({ ...processRange, to: event.target.value })
                  }
                />
              </label>
            </div>

            {rangeError ? (
              <p className="settings-error kr800-range__error" role="alert">
                {rangeError}
              </p>
            ) : null}
          </div>

          <div className="kr800-heading-actions">
            <button
              type="button"
              className="ds-button ds-button--ghost"
              onClick={() => setParamsOpen(true)}
              title="Cấu hình query params API danh sách người bệnh"
            >
              <SlidersHorizontal size={16} strokeWidth={2} aria-hidden="true" />
              Tham số
            </button>
            <button
              type="button"
              className="ds-button ds-button--ghost"
              onClick={() => setPatientsOpen(true)}
              disabled={!patientListReady}
              title={
                patientListReady
                  ? patientListMeta
                    ? `Xem JSON API người bệnh (${patientListMeta.patientCount.toLocaleString("vi-VN")} bản ghi)`
                    : "Xem JSON API danh sách người bệnh"
                  : "Chỉ bật sau khi gọi API danh sách bệnh nhân thành công (Xử lý / tự động xử lý)"
              }
            >
              <Users size={16} strokeWidth={2} aria-hidden="true" />
              Ds bệnh nhân
            </button>
            <button
              type="button"
              className="ds-button ds-button--primary kr800-process-button"
              onClick={onProcess}
              disabled={Boolean(processDisabledReason)}
              title={processDisabledReason || "Bắt đầu luồng xử lý KR-800"}
            >
              {processPhase === "running" ? (
                <Loader2 size={16} strokeWidth={2} className="spin" aria-hidden="true" />
              ) : (
                <Play size={16} strokeWidth={2} aria-hidden="true" />
              )}
              {processButtonLabel}
            </button>
          </div>
        </div>

        <div className="table-shell">
          <table>
            <thead>
              <tr>
                <th>Tên file</th>
                <th>Kích thước</th>
                <th>Trạng thái</th>
                <th>Ngày tạo</th>
                <th>Ngày cập nhật</th>
                <th>Lỗi</th>
              </tr>
            </thead>
            <tbody>
              {isLoading || isScanning ? (
                <tr>
                  <td colSpan={6} className="table-empty">
                    {isScanning ? (
                      <div className="kr800-scan-table-status">
                        <span>
                          {scanProgress?.message || "Đang quét thư mục tracking…"}
                        </span>
                        {scanProgress && scanProgress.total > 0 ? (
                          <span className="kr800-chip-muted">
                            {scanProgress.current.toLocaleString("vi-VN")}/
                            {scanProgress.total.toLocaleString("vi-VN")} ({scanProgress.percent}%)
                          </span>
                        ) : scanProgress && scanProgress.current > 0 ? (
                          <span className="kr800-chip-muted">
                            {scanProgress.current.toLocaleString("vi-VN")} file XML
                          </span>
                        ) : null}
                      </div>
                    ) : (
                      "Đang tải theo khoảng ngày tạo…"
                    )}
                  </td>
                </tr>
              ) : rangeError ? (
                <tr>
                  <td colSpan={6} className="table-empty">
                    Khoảng thời gian không hợp lệ — chỉnh lại bộ lọc phía trên.
                  </td>
                </tr>
              ) : !trackingFolder ? (
                <tr>
                  <td colSpan={6} className="table-empty">
                    Chọn thư mục tracking để tải danh sách file XML.
                  </td>
                </tr>
              ) : filteredFiles.length === 0 ? (
                <tr>
                  <td colSpan={6} className="table-empty">
                    Không có file XML có ngày tạo trong khoảng đã chọn. Đổi khoảng thời gian
                    hoặc bấm «Quét lại» nếu vừa xuất file mới.
                  </td>
                </tr>
              ) : (
                filteredFiles.map((file) => {
                  const active = isActiveFileStatus(file.status);
                  return (
                    <tr
                      key={file.id}
                      className={active ? "kr800-file-row is-processing" : undefined}
                    >
                      <td className="cell-file" title={file.filePath}>
                        {file.fileName}
                      </td>
                      <td>{formatSize(file.fileSize)}</td>
                      <td>
                        <span className={`status-badge track-${statusTone(file.status)}`}>
                          {active ? (
                            <Loader2
                              size={13}
                              strokeWidth={2.25}
                              className="spin"
                              aria-hidden="true"
                            />
                          ) : null}
                          {statusLabel[file.status]}
                        </span>
                      </td>
                      <td title={file.createdAt || undefined}>
                        {formatStoredDateTime(file.createdAt)}
                      </td>
                      <td title={file.updatedAt || undefined}>
                        {formatStoredDateTime(file.updatedAt)}
                      </td>
                      <td className="cell-error" title={file.errorMessage || undefined}>
                        {file.errorMessage || "—"}
                      </td>
                    </tr>
                  );
                })
              )}
            </tbody>
          </table>
        </div>
      </div>

      <PatientParamsDialog
        open={paramsOpen}
        onClose={() => setParamsOpen(false)}
        processRange={processRange}
      />
      <PatientListDialog open={patientsOpen} onClose={closePatientList} />
    </section>
  );
}

function ScanProgressBar({
  progress,
  compact = false,
}: {
  progress: Kr800ScanProgressEvent | null;
  compact?: boolean;
}) {
  const phaseLabel =
    progress?.phase === "disk"
      ? "Đọc disk"
      : progress?.phase === "index"
        ? "Ghi SQLite"
        : progress?.phase === "prune"
          ? "Dọn index"
          : progress?.phase === "done"
            ? "Hoàn tất"
            : "Đang quét";

  const hasTotal = Boolean(progress && progress.total > 0);
  const percent = hasTotal ? progress!.percent : undefined;
  const barStyle = hasTotal
    ? { width: `${Math.min(100, Math.max(0, percent ?? 0))}%` }
    : undefined;

  return (
    <div
      className={`kr800-scan-progress${compact ? " is-compact" : ""}`}
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={hasTotal ? 100 : undefined}
      aria-valuenow={hasTotal ? percent : undefined}
      aria-label={progress?.message || "Đang quét thư mục"}
    >
      <div className="kr800-scan-progress__meta">
        <strong>{phaseLabel}</strong>
        <span>
          {progress?.message || "Đang quét thư mục tracking…"}
          {hasTotal
            ? ` — ${progress!.current.toLocaleString("vi-VN")}/${progress!.total.toLocaleString("vi-VN")} (${percent}%)`
            : progress && progress.current > 0
              ? ` — ${progress.current.toLocaleString("vi-VN")} file XML`
              : ""}
        </span>
      </div>
      <div className={`kr800-scan-progress__track${hasTotal ? "" : " is-indeterminate"}`}>
        <div className="kr800-scan-progress__fill" style={barStyle} />
      </div>
    </div>
  );
}

/** Mặc định: hôm nay 00:00 → 23:59 (local). */
export function defaultProcessRange(): ProcessDateRange {
  const now = new Date();
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, "0");
  const d = String(now.getDate()).padStart(2, "0");
  return {
    from: `${y}-${m}-${d}T00:00`,
    to: `${y}-${m}-${d}T23:59`,
  };
}

/** Khôi phục khoảng ngày xử lý đã chọn; fallback về hôm nay nếu chưa có hoặc dữ liệu lỗi. */
export function loadStoredProcessRange(): ProcessDateRange {
  try {
    const raw = window.localStorage.getItem(PROCESS_RANGE_KEY);
    if (!raw) return defaultProcessRange();
    const stored = JSON.parse(raw) as Partial<ProcessDateRange>;
    if (isDateTimeLocalValue(stored.from) && isDateTimeLocalValue(stored.to)) {
      return { from: stored.from, to: stored.to };
    }
  } catch {
    // localStorage không khả dụng hoặc dữ liệu cũ không hợp lệ.
  }
  return defaultProcessRange();
}

/** Lưu ngay trong event thay đổi để không phụ thuộc effect/unmount của màn hình. */
export function saveStoredProcessRange(range: ProcessDateRange): void {
  try {
    window.localStorage.setItem(PROCESS_RANGE_KEY, JSON.stringify(range));
  } catch {
    // Ứng dụng vẫn dùng state hiện tại nếu localStorage không khả dụng.
  }
}

function isDateTimeLocalValue(value: unknown): value is string {
  return typeof value === "string" && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(?::\d{2})?$/.test(value);
}

/** Format HIS API / so sánh: `YYYY-MM-DD HH:mm:ss` (giây mặc định 00). */
export function toHisApiDateTime(value: string): string {
  return normalizeDateTimeBound(value, "start");
}

/** Mốc cuối khoảng — nếu không có giây thì lấy hết phút đó (`:59`). */
export function toFilterEndDateTime(value: string): string {
  return normalizeDateTimeBound(value, "end");
}

function normalizeDateTimeBound(value: string, bound: "start" | "end"): string {
  if (!value) return "";
  // datetime-local: 2026-07-06T00:00 or with seconds; SQLite: space separator
  const normalized = value.includes("T") ? value : value.replace(" ", "T");
  const [date, time = "00:00"] = normalized.split("T");
  const timeParts = time.split(":");
  const hh = timeParts[0] ?? "00";
  const mm = timeParts[1] ?? "00";
  const hasSeconds = timeParts.length >= 3 && timeParts[2] !== "";
  const ss = hasSeconds ? (timeParts[2] ?? "00") : bound === "end" ? "59" : "00";
  return `${date} ${hh.padStart(2, "0")}:${mm.padStart(2, "0")}:${String(ss).padStart(2, "0")}`;
}

/** Chuẩn hoá `created_at` lưu DB để so sánh lexicographic. */
export function normalizeStoredDateTime(value: string): string {
  if (!value) return "";
  return toHisApiDateTime(value.trim());
}

/**
 * Lọc file có `createdAt` nằm trong [from, to] (inclusive).
 * `from`/`to` dạng datetime-local `YYYY-MM-DDTHH:mm`.
 */
export function filterFilesByCreatedAt<T extends { createdAt?: string | null }>(
  files: T[],
  from: string,
  to: string,
): T[] {
  const fromNorm = toHisApiDateTime(from);
  const toNorm = toFilterEndDateTime(to);
  if (!fromNorm || !toNorm) return [];
  return files.filter((file) => {
    const created = normalizeStoredDateTime(file.createdAt ?? "");
    if (!created) return false;
    return created >= fromNorm && created <= toNorm;
  });
}

function formatRangeDisplay(value: string): string {
  if (!value) return "—";
  const api = toHisApiDateTime(value);
  const [date, time] = api.split(" ");
  if (!date || !time) return value;
  const [y, m, d] = date.split("-");
  return `${d}/${m}/${y} ${time.slice(0, 5)}`;
}

/** Hiển thị `YYYY-MM-DD HH:mm:ss` → `DD/MM/YYYY HH:mm`. */
function formatStoredDateTime(value?: string | null): string {
  if (!value) return "—";
  const api = normalizeStoredDateTime(value);
  const [date, time] = api.split(" ");
  if (!date || !time) return value;
  const [y, m, d] = date.split("-");
  return `${d}/${m}/${y} ${time.slice(0, 5)}`;
}

function countByStatus(files: TrackedXmlFile[]) {
  const counts = { waiting: 0, processing: 0, processed: 0, failed: 0 };
  for (const file of files) {
    if (file.status === "waiting" || file.status === "awaiting_pair") {
      counts.waiting += 1;
    } else if (file.status === "processed") {
      counts.processed += 1;
    } else if (
      file.status === "processing" ||
      file.status === "parsed" ||
      file.status === "patient_matched" ||
      file.status === "mapped" ||
      file.status === "sending" ||
      file.status === "pairing"
    ) {
      counts.processing += 1;
    } else {
      counts.failed += 1;
    }
  }
  return counts;
}

function statusTone(status: TrackedXmlStatus): "waiting" | "processing" | "processed" | "failed" {
  if (status === "waiting" || status === "awaiting_pair") return "waiting";
  if (status === "processed") return "processed";
  if (
    ["processing", "parsed", "patient_matched", "mapped", "sending", "pairing"].includes(status)
  ) {
    return "processing";
  }
  return "failed";
}

function isActiveFileStatus(status: TrackedXmlStatus): boolean {
  return ["processing", "parsed", "patient_matched", "mapped", "sending", "pairing"].includes(
    status,
  );
}

function formatSize(bytes?: number | null) {
  if (bytes == null || Number.isNaN(bytes)) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
