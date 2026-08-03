import { listen } from "@tauri-apps/api/event";
import { Eye, FolderOpen, Loader2, Play, RefreshCw, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  CT800_DEVICE_KEY,
  getCt800DeviceFolder,
  getCt800RevisionDetail,
  listCt800Files,
  pickTrackingFolder,
  processCt800,
  rescanTrackingFolder,
  setAutoProcessEnabled,
  setTrackingFolderAndScan,
} from "../../lib/appCommands";
import type { Ct800RevisionDetail, TrackedXmlFile, TrackedXmlStatus } from "../../types";
import {
  defaultProcessRange,
  toFilterEndDateTime,
  toHisApiDateTime,
  type ProcessDateRange,
} from "../kr800/Kr800Panel";

type ScanProgress = {
  phase: string;
  current: number;
  total: number;
  percent: number;
  message: string;
};

type AutoProcessEvent = { ok: boolean; message: string };
type WatchStatusEvent = { active: boolean; message: string };

const statusLabels: Record<TrackedXmlStatus, string> = {
  waiting: "Chờ xử lý",
  processing: "Đang xử lý",
  parsed: "Đã phân tích XML",
  patient_matched: "Đã tìm thấy bệnh nhân",
  mapped: "Đã mapping",
  sending: "Đang gửi HIS",
  processed: "Đã xử lý",
  patient_not_found: "Không tìm thấy bệnh nhân",
  treatment_ambiguous: "Không xác định đợt điều trị",
  service_not_found: "Không tìm thấy dịch vụ khám",
  xml_error: "Lỗi XML",
  mapping_error: "Lỗi mapping",
  send_error: "Lỗi gửi HIS",
  failed: "Thất bại",
  awaiting_pair: "Chờ lần đo 2",
  pairing: "Đang ghép cặp",
  pairing_error: "Lỗi ghép cặp",
  extra_measurement: "Lần đo thừa",
  duplicate: "Trùng nội dung",
  no_supported_data: "Không có dữ liệu nhãn áp",
  superseded: "Đã được file mới hơn thay thế",
  invalid_filename: "Tên file không hợp lệ",
};

export function Ct800Panel() {
  const [folder, setFolder] = useState<string | null>(null);
  const [files, setFiles] = useState<TrackedXmlFile[]>([]);
  const [range, setRange] = useState<ProcessDateRange>(defaultProcessRange);
  const [loading, setLoading] = useState(true);
  const [scanning, setScanning] = useState(false);
  const [processing, setProcessing] = useState(false);
  const [autoProcess, setAutoProcess] = useState(false);
  const [togglingAuto, setTogglingAuto] = useState(false);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [detail, setDetail] = useState<Ct800RevisionDetail | null>(null);
  const [detailLoadingId, setDetailLoadingId] = useState<number | null>(null);
  const rangeRef = useRef(range);
  const loadSequence = useRef(0);
  rangeRef.current = range;

  const load = useCallback(async () => {
    const sequence = ++loadSequence.current;
    setLoading(true);
    try {
      const from = toHisApiDateTime(rangeRef.current.from);
      const to = toFilterEndDateTime(rangeRef.current.to);
      const [state, items] = await Promise.all([
        getCt800DeviceFolder(),
        from && to
          ? listCt800Files(from, to)
          : Promise.resolve([] as TrackedXmlFile[]),
      ]);
      if (sequence !== loadSequence.current) return;
      setFolder(state.trackingFolder ?? null);
      setAutoProcess(Boolean(state.autoProcessEnabled));
      setFiles(items);
      setError(null);
    } catch (value) {
      if (sequence !== loadSequence.current) return;
      setError(messageFrom(value) || "Không tải được dữ liệu CT-800.");
    } finally {
      if (sequence === loadSequence.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load, range.from, range.to]);

  useEffect(() => {
    const listeners = [
      listen<TrackedXmlFile>("ct800:file-progress", ({ payload }) => {
        const from = toHisApiDateTime(rangeRef.current.from);
        const to = toFilterEndDateTime(rangeRef.current.to);
        const inside = payload.createdAt >= from && payload.createdAt <= to;
        setFiles((current) => {
          const exists = current.some((item) => item.id === payload.id);
          if (!inside) {
            return exists ? current.filter((item) => item.id !== payload.id) : current;
          }
          const next = exists
            ? current.map((item) => (item.id === payload.id ? payload : item))
            : [...current, payload];
          return next.sort((a, b) =>
            a.createdAt === b.createdAt ? a.id - b.id : a.createdAt.localeCompare(b.createdAt),
          );
        });
      }),
      listen<ScanProgress>("ct800:scan-progress", ({ payload }) => setProgress(payload)),
      listen<WatchStatusEvent>("ct800:watch-status", ({ payload }) =>
        setMessage(`${payload.active ? "●" : "○"} ${payload.message}`),
      ),
      listen<AutoProcessEvent>("ct800:auto-process", ({ payload }) => {
        setMessage(payload.message);
        void load();
      }),
      listen("ct800:files-indexed", () => void load()),
    ];
    return () => {
      listeners.forEach((listener) => void listener.then((dispose) => dispose()));
    };
  }, [load]);

  const counts = useMemo(() => {
    const value = { waiting: 0, processing: 0, processed: 0, failed: 0 };
    files.forEach((file) => {
      if (file.status === "waiting") value.waiting += 1;
      else if (["processing", "parsed", "patient_matched", "mapped", "sending"].includes(file.status)) {
        value.processing += 1;
      } else if (["processed", "duplicate", "no_supported_data", "superseded"].includes(file.status)) {
        value.processed += 1;
      } else value.failed += 1;
    });
    return value;
  }, [files]);

  async function chooseFolder() {
    const selected = await pickTrackingFolder("CT-800");
    if (!selected) return;
    setScanning(true);
    setError(null);
    setProgress({ phase: "disk", current: 0, total: 0, percent: 0, message: "Bắt đầu quét CT-800…" });
    try {
      const result = await setTrackingFolderAndScan(selected, CT800_DEVICE_KEY);
      setFolder(result.trackingFolder);
      setMessage(`Đã quét ${result.scannedCount} file; thêm ${result.insertedCount} revision CT-800.`);
      await load();
    } catch (value) {
      setError(messageFrom(value) || "Không chọn được thư mục CT-800.");
    } finally {
      setScanning(false);
      setProgress(null);
    }
  }

  async function rescan() {
    setScanning(true);
    setError(null);
    setProgress({ phase: "disk", current: 0, total: 0, percent: 0, message: "Bắt đầu quét lại CT-800…" });
    try {
      const result = await rescanTrackingFolder(CT800_DEVICE_KEY);
      setMessage(`Đã quét lại ${result.scannedCount} file; thêm ${result.insertedCount} revision.`);
      await load();
    } catch (value) {
      setError(messageFrom(value) || "Không quét lại được CT-800.");
    } finally {
      setScanning(false);
      setProgress(null);
    }
  }

  async function processFiles() {
    const from = toHisApiDateTime(range.from);
    const to = toFilterEndDateTime(range.to);
    setProcessing(true);
    setError(null);
    try {
      const result = await processCt800(from, to);
      if (
        toHisApiDateTime(rangeRef.current.from) === from &&
        toFilterEndDateTime(rangeRef.current.to) === to
      ) {
        setFiles(result.files);
      } else {
        void load();
      }
      setMessage(
        result.total === 0
          ? "Không có file CT-800 chờ xử lý."
          : `Đã xử lý ${result.processed}/${result.total}; bỏ qua ${result.skipped}; lỗi ${result.failed}.`,
      );
    } catch (value) {
      setError(messageFrom(value) || "Không xử lý được CT-800.");
    } finally {
      setProcessing(false);
    }
  }

  async function toggleAutoProcess() {
    if (togglingAuto) return;
    setTogglingAuto(true);
    setError(null);
    try {
      const state = await setAutoProcessEnabled(!autoProcess, CT800_DEVICE_KEY);
      setAutoProcess(Boolean(state.autoProcessEnabled));
      setMessage(
        state.autoProcessEnabled
          ? "Đã bật tự động xử lý CT-800."
          : "Đã tắt tự động xử lý CT-800.",
      );
    } catch (value) {
      setError(messageFrom(value) || "Không lưu được cấu hình tự động xử lý.");
    } finally {
      setTogglingAuto(false);
    }
  }

  async function showDetail(id: number) {
    setDetailLoadingId(id);
    setError(null);
    try {
      setDetail(await getCt800RevisionDetail(id));
    } catch (value) {
      setError(messageFrom(value) || "Không đọc được chi tiết revision CT-800.");
    } finally {
      setDetailLoadingId(null);
    }
  }

  return (
    <section className="kr800-panel panel-stack">
      <div className="kr800-folder ds-card">
        <div className="panel-heading kr800-folder__heading">
          <div>
            <h2>Thư mục tracking CT-800</h2>
            <p className="panel-lead">
              Mỗi XML hợp lệ được gửi độc lập. Chỉ dùng Average/IOP_mmHg và timestamp trong tên file.
            </p>
          </div>
          <div className="panel-actions">
            <button className="ds-button ds-button--ghost" onClick={() => void rescan()} disabled={!folder || scanning}>
              <RefreshCw size={16} className={scanning ? "spin" : undefined} />Quét lại
            </button>
            <button className="ds-button ds-button--primary" onClick={() => void chooseFolder()} disabled={scanning}>
              <FolderOpen size={16} />Chọn thư mục
            </button>
          </div>
        </div>
        <div className="path-box"><span>Đường dẫn đang theo dõi</span><strong>{folder || "Chưa chọn thư mục"}</strong></div>
        <div className="kr800-status-row">
          <span className="sync-summary-chip">Chờ <strong>{counts.waiting}</strong></span>
          <span className="sync-summary-chip">Đang xử lý <strong>{counts.processing}</strong></span>
          <span className="sync-summary-chip">Hoàn tất <strong>{counts.processed}</strong></span>
          <span className="sync-summary-chip">Lỗi <strong>{counts.failed}</strong></span>
          <button className="ds-button ds-button--ghost" onClick={() => void toggleAutoProcess()} disabled={togglingAuto}>
            {autoProcess ? "Tắt tự động xử lý" : "Bật tự động xử lý"}
          </button>
        </div>
        {progress ? <p className="kr800-message">{progress.message}</p> : null}
        {message ? <p className="kr800-message">{message}</p> : null}
        {error ? <p className="settings-error">{error}</p> : null}
      </div>

      <div className="kr800-table-block">
        <div className="panel-heading kr800-table-block__heading">
          <h2>Danh sách XML CT-800</h2>
          <div className="kr800-heading-actions">
            <label className="kr800-range__field"><span>Từ</span><input type="datetime-local" value={range.from} onChange={(event) => setRange({ ...range, from: event.target.value })} /></label>
            <label className="kr800-range__field"><span>Đến</span><input type="datetime-local" value={range.to} onChange={(event) => setRange({ ...range, to: event.target.value })} /></label>
            <button className="ds-button ds-button--primary" onClick={() => void processFiles()} disabled={processing || loading || scanning}>
              {processing ? <Loader2 size={16} className="spin" /> : <Play size={16} />}
              {processing ? "Đang xử lý…" : "Xử lý"}
            </button>
          </div>
        </div>
        <div className="table-shell">
          <table>
            <thead><tr><th>Tên file</th><th>Trạng thái</th><th>Timestamp nguồn</th><th>Cập nhật</th><th>Lỗi</th><th>Chi tiết</th></tr></thead>
            <tbody>
              {loading ? <tr><td colSpan={6} className="table-empty">Đang tải…</td></tr> : files.length === 0 ? <tr><td colSpan={6} className="table-empty">Không có XML CT-800 trong khoảng đã chọn.</td></tr> : files.map((file) => <tr key={file.id}><td title={file.filePath}>{file.fileName}</td><td><span className="status-badge">{statusLabels[file.status]}</span></td><td>{file.createdAt}</td><td>{file.updatedAt}</td><td className="cell-error">{file.errorMessage || "—"}</td><td><button className="ds-button ds-button--ghost" onClick={() => void showDetail(file.id)} disabled={detailLoadingId !== null}>{detailLoadingId === file.id ? <Loader2 size={15} className="spin" /> : <Eye size={15} />}Xem</button></td></tr>)}
            </tbody>
          </table>
        </div>
      </div>

      {detail ? (
        <div className="ds-card">
          <div className="panel-heading">
            <div>
              <h2>Chi tiết revision CT-800 #{detail.id}</h2>
              <p className="panel-lead">{detail.fileName}</p>
            </div>
            <button className="ds-button ds-button--ghost" onClick={() => setDetail(null)}><X size={16} />Đóng</button>
          </div>
          <div className="kr800-status-row">
            <span className="sync-summary-chip">Hồ sơ <strong>{detail.maHoSo || "—"}</strong></span>
            <span className="sync-summary-chip">Nguồn <strong>{detail.sourceTime || "—"}</strong></span>
            <span className="sync-summary-chip">Serial <strong>{detail.machineSerial || "—"}</strong></span>
            <span className="sync-summary-chip">dvKhamId <strong>{detail.dvKhamId ?? "—"}</strong></span>
            <span className="sync-summary-chip">Lần gửi <strong>{detail.attemptCount}</strong></span>
          </div>
          <p className="kr800-message">
            IOP phải: {detail.rawRightIop ?? "rỗng"} → {detail.rightIopId ?? "—"} · IOP trái: {detail.rawLeftIop ?? "rỗng"} → {detail.leftIopId ?? "—"} · XML: {detail.xmlTime || "—"}
          </p>
          <p className="kr800-message">
            Model XML: {detail.xmlModel || "—"} · SHA-256: {detail.contentHash} · Trạng thái: {detail.status}
          </p>
          <div className="patient-json-panel">
            <div className="patient-json-panel__toolbar"><span className="patient-json-panel__label">Request JSON</span></div>
            <pre className="patient-json-view" tabIndex={0}>{prettyJson(detail.requestPayload)}</pre>
          </div>
          <div className="patient-json-panel">
            <div className="patient-json-panel__toolbar"><span className="patient-json-panel__label">Response HIS</span></div>
            <pre className="patient-json-view" tabIndex={0}>{prettyJson(detail.responsePayload)}</pre>
          </div>
          {detail.errorMessage ? <p className="settings-error">{detail.errorMessage}</p> : null}
        </div>
      ) : null}
    </section>
  );
}

function prettyJson(value?: string | null): string {
  if (!value) return "Chưa có dữ liệu.";
  try {
    return JSON.stringify(JSON.parse(value), null, 2);
  } catch {
    return value;
  }
}

function messageFrom(value: unknown): string {
  if (typeof value === "string") return value;
  if (value && typeof value === "object" && "message" in value && typeof value.message === "string") {
    return value.message;
  }
  return "";
}
