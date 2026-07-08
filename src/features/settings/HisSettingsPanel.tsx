import { PlugZap, Save } from "lucide-react";
import type { AppSettings } from "../../types";

type HisSettingsPanelProps = {
  settings: AppSettings;
  onChange: (settings: AppSettings) => void;
  onSave: () => void;
  onTestConnection: () => void;
  connectionLabel: string;
};

export function HisSettingsPanel({
  settings,
  onChange,
  onSave,
  onTestConnection,
  connectionLabel,
}: HisSettingsPanelProps) {
  return (
    <section className="settings-layout">
      <div className="form-panel ds-card">
        <div className="panel-heading">
          <div>
            <h2>Cấu hình kết nối HIS</h2>
            <p>Thông tin này sẽ được backend dùng khi login, lấy danh sách người bệnh và gửi kết quả.</p>
          </div>
        </div>

        <div className="form-grid">
          <label className="field">
            <span>Base URL</span>
            <input
              value={settings.apiBaseUrl}
              onChange={(event) => onChange({ ...settings, apiBaseUrl: event.target.value })}
            />
          </label>

          <label className="field">
            <span>Tài khoản</span>
            <input
              value={settings.username}
              placeholder="Nhập tài khoản HIS"
              onChange={(event) => onChange({ ...settings, username: event.target.value })}
            />
          </label>

          <label className="field">
            <span>Mật khẩu / token policy</span>
            <input type="password" placeholder="Sẽ lưu bằng secure storage ở bước backend" />
          </label>

          <label className="field">
            <span>coSoKcbId</span>
            <input
              type="number"
              value={settings.facilityId ?? ""}
              onChange={(event) =>
                onChange({
                  ...settings,
                  facilityId: event.target.value ? Number(event.target.value) : null,
                })
              }
            />
          </label>
        </div>

        <div className="button-row">
          <button type="button" className="ds-button ds-button--ghost" onClick={onTestConnection}>
            <PlugZap size={16} strokeWidth={2} aria-hidden="true" />
            Kiểm tra kết nối
          </button>
          <button type="button" className="ds-button ds-button--primary" onClick={onSave}>
            <Save size={16} strokeWidth={2} aria-hidden="true" />
            Lưu cấu hình
          </button>
        </div>
      </div>

      <aside className="side-note">
        <span>Trạng thái</span>
        <strong>{connectionLabel}</strong>
        <p>Form đã đặt đúng contract để nối `get_settings` và `save_settings` từ Tauri.</p>
      </aside>
    </section>
  );
}
