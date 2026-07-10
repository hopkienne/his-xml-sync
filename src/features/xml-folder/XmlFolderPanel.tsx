import { Folder, Info, RefreshCw } from "lucide-react";
import type { XmlFolderState } from "../../types";

type XmlFolderPanelProps = {
  state: XmlFolderState;
  onChange: (state: XmlFolderState) => void;
  xmlFileCount: number;
  latestXmlFile?: string;
  onPickFolder: () => void;
  onRefresh?: () => void;
};

export function XmlFolderPanel({
  state,
  onChange,
  xmlFileCount,
  latestXmlFile,
  onPickFolder,
  onRefresh,
}: XmlFolderPanelProps) {
  return (
    <section className="folder-layout">
      <div className="folder-summary ds-card">
        <div className="panel-heading">
          <div>
            <h2>Thư mục XML</h2>
            <p className="panel-lead">
              Nơi máy TOPCON KR-800 xuất file kết quả đo khúc xạ. Ứng dụng quét và đồng bộ từ đường
              dẫn này.
            </p>
          </div>
          <div className="panel-actions">
            {onRefresh ? (
              <button type="button" className="ds-button ds-button--ghost" onClick={onRefresh}>
                <RefreshCw size={16} strokeWidth={2} aria-hidden="true" />
                Làm mới
              </button>
            ) : null}
            <button type="button" className="ds-button ds-button--primary" onClick={onPickFolder}>
              <Folder size={16} strokeWidth={2} aria-hidden="true" />
              Chọn thư mục
            </button>
          </div>
        </div>

        <div className="path-box">
          <span>Đường dẫn hiện tại</span>
          <strong title={state.xmlFolder || undefined}>
            {state.xmlFolder || "Chưa chọn thư mục"}
          </strong>
        </div>

        <div className="folder-metrics">
          <div>
            <span>File XML tìm thấy</span>
            <strong>{xmlFileCount}</strong>
          </div>
          <div>
            <span>File mới nhất</span>
            <strong title={latestXmlFile}>{latestXmlFile || "—"}</strong>
          </div>
        </div>

        <label className="toggle-row">
          <input
            type="checkbox"
            checked={state.autoSyncEnabled}
            onChange={(event) =>
              onChange({ ...state, autoSyncEnabled: event.currentTarget.checked })
            }
          />
          <span>Tự động đồng bộ khi có file XML mới</span>
        </label>
      </div>

      <aside className="side-note">
        <div className="side-note__icon" aria-hidden="true">
          <Info size={16} strokeWidth={2} />
        </div>
        <span>Folder picker</span>
        <strong>Chưa lưu SQLite</strong>
        <p>
          Thư mục XML sẽ được persist ở bước sau. Hiện chỉ giữ local UI; cấu hình kết nối API đã lưu
          bảng <code>app_config</code>.
        </p>
      </aside>
    </section>
  );
}
