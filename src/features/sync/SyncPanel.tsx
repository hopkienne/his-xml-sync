import { RefreshCw, UploadCloud } from "lucide-react";
import type { SyncFileRow, SyncSummary } from "../../types";

type SyncPanelProps = {
  rows: SyncFileRow[];
  selectedId: string | null;
  onSelect: (id: string) => void;
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

export function SyncPanel({
  rows,
  selectedId,
  onSelect,
  summary,
  onRunSync,
  isSyncing,
}: SyncPanelProps) {
  const selected = rows.find((row) => row.id === selectedId) ?? rows[0];

  return (
    <section className="sync-layout">
      <div className="sync-main">
        <div className="panel-heading">
          <div>
            <h2>Danh sách file XML</h2>
            <p className="panel-lead">
              Kiểm tra patient ID, thời gian đo và trạng thái gửi HIS. Chọn một dòng để xem preview
              mắt phải / mắt trái.
            </p>
          </div>
          <div className="panel-actions">
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
        </div>

        {summary ? (
          <div className="sync-summary-bar" aria-live="polite">
            <span className="sync-summary-chip">
              Quét <strong>{summary.scannedFiles}</strong>
            </span>
            <span className="sync-summary-chip">
              Gửi <strong>{summary.sentResults}</strong>
            </span>
            <span className="sync-summary-chip">
              Bỏ qua <strong>{summary.skippedFiles}</strong>
            </span>
            <span className="sync-summary-chip">
              Lỗi <strong>{summary.failedFiles}</strong>
            </span>
          </div>
        ) : null}

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
              {rows.map((row) => {
                const isSelected = selected?.id === row.id;
                return (
                  <tr
                    key={row.id}
                    className={isSelected ? "is-selected" : undefined}
                    onClick={() => onSelect(row.id)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault();
                        onSelect(row.id);
                      }
                    }}
                    tabIndex={0}
                    aria-selected={isSelected}
                  >
                    <td className="cell-file" title={row.fileName}>
                      {row.fileName}
                    </td>
                    <td>{row.patientId}</td>
                    <td>{row.measuredAt}</td>
                    <td>
                      <span className={`status-badge ${row.status}`}>
                        {statusCopy[row.status]}
                      </span>
                    </td>
                    <td className="cell-error" title={row.error}>
                      {row.error ?? "—"}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </div>

      <aside className="preview-panel ds-card">
        <h3>Preview kết quả</h3>
        {selected ? (
          <>
            <div className="preview-panel__meta">
              <span>File đang chọn</span>
              <strong title={selected.fileName}>{selected.fileName}</strong>
              <span>Patient · {selected.patientId}</span>
            </div>
            <div className="eye-grid">
              <EyePreview title="Mắt phải (OD)" value={selected.right} />
              <EyePreview title="Mắt trái (OS)" value={selected.left} />
            </div>
            <p style={{ marginTop: 14 }}>
              Preview map với command <code>preview_xml_file</code> khi parse XML thật.
            </p>
          </>
        ) : (
          <p>Chưa có file để preview.</p>
        )}
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
          <dd>{value.sphere || "—"}</dd>
        </div>
        <div>
          <dt>CYL</dt>
          <dd>{value.cylinder || "—"}</dd>
        </div>
        <div>
          <dt>AX</dt>
          <dd>{value.axis || "—"}</dd>
        </div>
      </dl>
    </div>
  );
}
