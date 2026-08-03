import { listen } from "@tauri-apps/api/event";
import { FolderOpen, Loader2, Play, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { HDR9000_DEVICE_KEY, getHdr9000DeviceFolder, listHdr9000Files, pickTrackingFolder, processHdr9000, rescanTrackingFolder, setAutoProcessEnabled, setTrackingFolderAndScan } from "../../lib/appCommands";
import type { TrackedXmlFile, TrackedXmlStatus } from "../../types";
import { defaultProcessRange, toFilterEndDateTime, toHisApiDateTime, type ProcessDateRange } from "../kr800/Kr800Panel";

type ScanProgress = { phase: string; current: number; total: number; percent: number; message: string };
type AutoEvent = { ok: boolean; message: string };
type WatchEvent = { active: boolean; message: string };
const labels: Record<TrackedXmlStatus, string> = {
  waiting: "Chờ xử lý", processing: "Đang xử lý", parsed: "Đã phân tích XML", patient_matched: "Đã tìm thấy bệnh nhân", mapped: "Đã mapping", sending: "Đang gửi HIS", processed: "Đã xử lý", patient_not_found: "Không tìm thấy bệnh nhân", treatment_ambiguous: "Không xác định đợt điều trị", service_not_found: "Không tìm thấy dịch vụ khám", xml_error: "Lỗi XML", mapping_error: "Lỗi mapping", send_error: "Lỗi gửi HIS", failed: "Thất bại", awaiting_pair: "Chờ lần đo 2", pairing: "Đang ghép cặp", pairing_error: "Lỗi ghép cặp", extra_measurement: "Lần đo thừa", duplicate: "Trùng nội dung", no_supported_data: "Không có dữ liệu hỗ trợ", superseded: "Đã được revision mới hơn thay thế", invalid_filename: "Tên file không hợp lệ",
};

export function Hdr9000Panel() {
  const [folder, setFolder] = useState<string | null>(null);
  const [files, setFiles] = useState<TrackedXmlFile[]>([]);
  const [range, setRange] = useState<ProcessDateRange>(defaultProcessRange);
  const [loading, setLoading] = useState(true);
  const [scanning, setScanning] = useState(false);
  const [processing, setProcessing] = useState(false);
  const [autoProcess, setAutoProcess] = useState(false);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const rangeRef = useRef(range);
  const loadSequence = useRef(0);
  rangeRef.current = range;

  const load = useCallback(async () => {
    const sequence = ++loadSequence.current;
    setLoading(true);
    try {
      const from = toHisApiDateTime(rangeRef.current.from);
      const to = toFilterEndDateTime(rangeRef.current.to);
      const [state, items] = await Promise.all([getHdr9000DeviceFolder(), from && to ? listHdr9000Files(from, to) : Promise.resolve([] as TrackedXmlFile[])]);
      if (sequence !== loadSequence.current) return;
      setFolder(state.trackingFolder ?? null);
      setAutoProcess(Boolean(state.autoProcessEnabled));
      setFiles(items);
      setError(null);
    } catch (value) {
      if (sequence !== loadSequence.current) return;
      setError(messageFrom(value) || "Không tải được dữ liệu HDR-9000.");
    } finally { if (sequence === loadSequence.current) setLoading(false); }
  }, []);

  useEffect(() => { void load(); }, [load, range.from, range.to]);
  useEffect(() => {
    const listeners = [
      listen<TrackedXmlFile>("hdr9000:file-progress", ({ payload }) => {
        const from = toHisApiDateTime(rangeRef.current.from);
        const to = toFilterEndDateTime(rangeRef.current.to);
        const inside = payload.createdAt >= from && payload.createdAt <= to;
        setFiles((current) => {
          const exists = current.some((item) => item.id === payload.id);
          if (!inside) return exists ? current.filter((item) => item.id !== payload.id) : current;
          return exists ? current.map((item) => item.id === payload.id ? payload : item) : [...current, payload];
        });
      }),
      listen<ScanProgress>("hdr9000:scan-progress", ({ payload }) => setProgress(payload)),
      listen<WatchEvent>("hdr9000:watch-status", ({ payload }) => setMessage((payload.active ? "● " : "○ ") + payload.message)),
      listen<AutoEvent>("hdr9000:auto-process", ({ payload }) => { setMessage(payload.message); void load(); }),
      listen("hdr9000:files-indexed", () => void load()),
    ];
    return () => { listeners.forEach((listener) => void listener.then((dispose) => dispose())); };
  }, [load]);

  const counts = useMemo(() => {
    const value = { waiting: 0, processing: 0, processed: 0, failed: 0 };
    files.forEach((file) => {
      if (file.status === "waiting") value.waiting += 1;
      else if (["processing", "parsed", "patient_matched", "mapped", "sending"].includes(file.status)) value.processing += 1;
      else if (["processed", "duplicate", "no_supported_data", "superseded"].includes(file.status)) value.processed += 1;
      else value.failed += 1;
    });
    return value;
  }, [files]);

  async function chooseFolder() {
    const selected = await pickTrackingFolder("HDR-9000");
    if (!selected) return;
    setScanning(true); setError(null);
    try {
      const result = await setTrackingFolderAndScan(selected, HDR9000_DEVICE_KEY);
      setFolder(result.trackingFolder);
      setMessage("Đã quét " + result.scannedCount + " file; thêm " + result.insertedCount + " revision HDR-9000.");
      await load();
    } catch (value) { setError(messageFrom(value) || "Không chọn được thư mục HDR-9000."); }
    finally { setScanning(false); }
  }

  async function rescan() {
    setScanning(true); setError(null);
    try {
      const result = await rescanTrackingFolder(HDR9000_DEVICE_KEY);
      setMessage("Đã quét lại " + result.scannedCount + " file; thêm " + result.insertedCount + " revision.");
      await load();
    } catch (value) { setError(messageFrom(value) || "Không quét lại được HDR-9000."); }
    finally { setScanning(false); }
  }

  async function process() {
    setProcessing(true); setError(null);
    try {
      const from = toHisApiDateTime(range.from);
      const to = toFilterEndDateTime(range.to);
      const result = await processHdr9000(from, to);
      if (toHisApiDateTime(rangeRef.current.from) === from && toFilterEndDateTime(rangeRef.current.to) === to) {
        setFiles(result.files);
      } else {
        void load();
      }
      setMessage(result.total === 0 ? "Không có revision HDR-9000 chờ xử lý." : "Đã xử lý " + result.processed + "/" + result.total + "; bỏ qua " + result.skipped + "; lỗi " + result.failed + ".");
    } catch (value) { setError(messageFrom(value) || "Không xử lý được HDR-9000."); }
    finally { setProcessing(false); }
  }

  async function toggleAuto() {
    try {
      const state = await setAutoProcessEnabled(!autoProcess, HDR9000_DEVICE_KEY);
      setAutoProcess(Boolean(state.autoProcessEnabled));
      setMessage(state.autoProcessEnabled ? "Đã bật tự động xử lý HDR-9000." : "Đã tắt tự động xử lý HDR-9000.");
    } catch (value) { setError(messageFrom(value) || "Không lưu được cấu hình tự động xử lý."); }
  }

  return (
    <section className="kr800-panel panel-stack">
      <div className="kr800-folder ds-card">
        <div className="panel-heading kr800-folder__heading">
          <div><h2>Thư mục tracking HDR-9000</h2><p className="panel-lead">Chỉ nhận XML có Product_Model là HDR-9000. Nội dung mới ở cùng đường dẫn là revision độc lập.</p></div>
          <div className="panel-actions">
            <button className="ds-button ds-button--ghost" onClick={() => void rescan()} disabled={!folder || scanning}><RefreshCw size={16} className={scanning ? "spin" : undefined} />Quét lại</button>
            <button className="ds-button ds-button--primary" onClick={() => void chooseFolder()} disabled={scanning}><FolderOpen size={16} />Chọn thư mục</button>
          </div>
        </div>
        <div className="path-box"><span>Đường dẫn đang theo dõi</span><strong>{folder || "Chưa chọn thư mục"}</strong></div>
        <div className="kr800-status-row">
          <span className="sync-summary-chip">Chờ <strong>{counts.waiting}</strong></span><span className="sync-summary-chip">Đang xử lý <strong>{counts.processing}</strong></span><span className="sync-summary-chip">Hoàn tất <strong>{counts.processed}</strong></span><span className="sync-summary-chip">Lỗi <strong>{counts.failed}</strong></span>
          <button className="ds-button ds-button--ghost" onClick={() => void toggleAuto()}>{autoProcess ? "Tắt tự động xử lý" : "Bật tự động xử lý"}</button>
        </div>
        {progress ? <p className="kr800-message">{progress.message}</p> : null}{message ? <p className="kr800-message">{message}</p> : null}{error ? <p className="settings-error">{error}</p> : null}
      </div>
      <div className="kr800-table-block"><div className="panel-heading kr800-table-block__heading"><h2>Danh sách revision XML</h2><div className="kr800-heading-actions">
        <label className="kr800-range__field"><span>Từ</span><input type="datetime-local" value={range.from} onChange={(event) => setRange({ ...range, from: event.target.value })} /></label>
        <label className="kr800-range__field"><span>Đến</span><input type="datetime-local" value={range.to} onChange={(event) => setRange({ ...range, to: event.target.value })} /></label>
        <button className="ds-button ds-button--primary" onClick={() => void process()} disabled={!folder || processing || loading || scanning}>{processing ? <Loader2 size={16} className="spin" /> : <Play size={16} />}{processing ? "Đang xử lý…" : "Xử lý"}</button>
      </div></div>
      <div className="table-shell"><table><thead><tr><th>Tên file</th><th>Trạng thái</th><th>Ngày lọc</th><th>Cập nhật</th><th>Lỗi</th></tr></thead><tbody>
        {loading ? <tr><td colSpan={5} className="table-empty">Đang tải…</td></tr> : files.length === 0 ? <tr><td colSpan={5} className="table-empty">Không có revision HDR-9000 trong khoảng đã chọn.</td></tr> : files.map((file) => <tr key={file.id}><td title={file.filePath}>{file.fileName}</td><td><span className="status-badge">{labels[file.status]}</span></td><td>{file.createdAt}</td><td>{file.updatedAt}</td><td className="cell-error">{file.errorMessage || "—"}</td></tr>)}
      </tbody></table></div></div>
    </section>
  );
}

function messageFrom(value: unknown): string {
  if (typeof value === "string") return value;
  if (value && typeof value === "object" && "message" in value && typeof value.message === "string") return value.message;
  return "";
}
