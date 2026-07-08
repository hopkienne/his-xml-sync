import { Folder, RefreshCw } from "lucide-react";
import type { AppSettings } from "../../types";

type XmlFolderPanelProps = {
  settings: AppSettings;
  onChange: (settings: AppSettings) => void;
};

export function XmlFolderPanel({ settings, onChange }: XmlFolderPanelProps) {
  return (
    <section className="folder-layout">
      <div className="folder-summary ds-card">
        <div className="panel-heading">
          <div>
            <h2>Thư mục XML</h2>
            <p>Nơi máy TOPCON KR-800 xuất file kết quả đo khúc xạ.</p>
          </div>
          <button type="button" className="ds-button ds-button--ghost">
            <Folder size={16} strokeWidth={2} aria-hidden="true" />
            Chọn thư mục
          </button>
        </div>

        <div className="path-box">
          <span>Đường dẫn hiện tại</span>
          <strong>{settings.xmlFolder || "Chưa chọn thư mục"}</strong>
        </div>

        <div className="folder-metrics">
          <div>
            <span>File XML tìm thấy</span>
            <strong>12</strong>
          </div>
          <div>
            <span>File mới nhất</span>
            <strong>HCM2607070269_20260707_145000_TOPCON_KR-800_4780634.xml</strong>
          </div>
        </div>

        <label className="toggle-row">
          <input
            type="checkbox"
            checked={settings.autoSyncEnabled}
            onChange={(event) =>
              onChange({ ...settings, autoSyncEnabled: event.currentTarget.checked })
            }
          />
          <span>Tự động đồng bộ khi có file XML mới</span>
        </label>
      </div>

      <aside className="side-note">
        <RefreshCw size={18} strokeWidth={2} aria-hidden="true" />
        <strong>Folder picker</strong>
        <p>Vị trí nút chọn thư mục đã sẵn sàng; bước sau chỉ cần nối plugin dialog của Tauri.</p>
      </aside>
    </section>
  );
}
