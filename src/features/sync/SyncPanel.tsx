import { RefreshCw, UploadCloud } from "lucide-react";
import type { SyncFileRow, SyncSummary } from "../../types";

type SyncPanelProps = {
  rows: SyncFileRow[];
  summary?: SyncSummary;
  onRunSync: () => void;
  isSyncing: boolean;
};

const statusCopy: Record<SyncFileRow["status"], string> = {
  waiting: "Chờ xử lý",
  sent: "Đã gửi",
  "needs-review": "Cần kiểm tra",
  failed: "Lỗi",
};

export function SyncPanel({ rows, summary, onRunSync, isSyncing }: SyncPanelProps) {
  const selected = rows[0];

  return (
    <section className="sync-layout">
      <div className="sync-main">
        <div className="panel-heading">
          <div>
            <h2>Danh sách file XML</h2>
            <p>Kiểm tra patient ID, thời gian đo và trạng thái gửi HIS.</p>
          </div>
          <button
            type="button"
            className="ds-button ds-button--primary"
            onClick={onRunSync}
            disabled={isSyncing}
          >
            {isSyncing ? (
              <RefreshCw size={16} strokeWidth={2} className="spin" aria-hidden="true" />
            ) : (
              <UploadCloud size={16} strokeWidth={2} aria-hidden="true" />
            )}
            Đồng bộ ngay
          </button>
        </div>

        <div className="table-shell">
          <table>
            <thead>
              <tr>
                <th>Tên file</th>
                <th>Patient ID</th>
                <th>Thời gian đo</th>
                <th>Trạng thái</th>
                <th>Lỗi</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={row.id}>
                  <td>{row.fileName}</td>
                  <td>{row.patientId}</td>
                  <td>{row.measuredAt}</td>
                  <td>
                    <span className={`status-badge ${row.status}`}>{statusCopy[row.status]}</span>
                  </td>
                  <td>{row.error ?? "-"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>

      <aside className="preview-panel ds-card">
        <h3>Preview kết quả</h3>
        {summary ? (
          <p>
            Quét {summary.scannedFiles} file, gửi {summary.sentResults}, bỏ qua{" "}
            {summary.skippedFiles}, lỗi {summary.failedFiles}.
          </p>
        ) : (
          <p>Dữ liệu mẫu từ file mới nhất, dùng để đặt layout preview trước khi gửi.</p>
        )}
        <div className="eye-grid">
          <EyePreview title="Mắt phải" value={selected.right} />
          <EyePreview title="Mắt trái" value={selected.left} />
        </div>
      </aside>
    </section>
  );
}

function EyePreview({
  title,
  value,
}: {
  title: string;
  value: SyncFileRow["right"];
}) {
  return (
    <div className="eye-preview">
      <strong>{title}</strong>
      <dl>
        <div>
          <dt>SPH</dt>
          <dd>{value.sphere}</dd>
        </div>
        <div>
          <dt>CYL</dt>
          <dd>{value.cylinder}</dd>
        </div>
        <div>
          <dt>AX</dt>
          <dd>{value.axis}</dd>
        </div>
      </dl>
    </div>
  );
}
