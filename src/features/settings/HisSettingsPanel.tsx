import { Download, Info, Loader2, PlugZap, Save } from "lucide-react";
import type { AppLogInfo, AppSettings, HisAuthStatus } from "../../types";

type HisSettingsPanelProps = {
  settings: AppSettings;
  onChange: (settings: AppSettings) => void;
  onSave: () => void;
  onTestConnection: () => void;
  onExportLogs: () => void;
  connectionLabel: string;
  isSaving?: boolean;
  isTestingConnection?: boolean;
  isProcessing?: boolean;
  isExportingLogs?: boolean;
  saveError?: string | null;
  logInfo?: AppLogInfo | null;
  logStatus?: string | null;
  logError?: string | null;
  hisAuth?: HisAuthStatus | null;
  hisAuthError?: string | null;
  /** Tự động xử lý KR-800 (lưu device_config, áp dụng ngay khi đổi). */
  autoProcessEnabled?: boolean;
  isTogglingAutoProcess?: boolean;
  trackingFolder?: string | null;
  autoProcessError?: string | null;
  autoProcessStatus?: string | null;
  onAutoProcessChange?: (enabled: boolean) => void;
  /** Khởi động cùng Windows (đăng ký OS). */
  autostartEnabled?: boolean;
  isTogglingAutostart?: boolean;
  autostartError?: string | null;
  autostartStatus?: string | null;
  onAutostartChange?: (enabled: boolean) => void;
};

export function HisSettingsPanel({
  settings,
  onChange,
  onSave,
  onTestConnection,
  onExportLogs,
  connectionLabel,
  isSaving = false,
  isTestingConnection = false,
  isProcessing = false,
  isExportingLogs = false,
  saveError = null,
  logInfo = null,
  logStatus = null,
  logError = null,
  hisAuth = null,
  hisAuthError = null,
  autoProcessEnabled = false,
  isTogglingAutoProcess = false,
  trackingFolder = null,
  autoProcessError = null,
  autoProcessStatus = null,
  onAutoProcessChange,
  autostartEnabled = false,
  isTogglingAutostart = false,
  autostartError = null,
  autostartStatus = null,
  onAutostartChange,
}: HisSettingsPanelProps) {
  const hasStoredPassword = Boolean(settings.hasPassword);

  return (
    <section className="settings-layout">
      <div className="settings-main-stack">
        <div className="form-panel ds-card">
          <div className="panel-heading">
            <div>
              <h2>Cấu hình kết nối</h2>

            </div>
          </div>

          <div className="form-grid">
            <label className="field form-grid--full">
              <span>API URL HIS (base)</span>
              <input
                value={settings.hisApiUrl}
                placeholder="https://api-hisvn.vietngagroup.vn"
                onChange={(event) => onChange({ ...settings, hisApiUrl: event.target.value })}
                autoComplete="off"
                spellCheck={false}
              />
            </label>

            <label className="field">
              <span>Tài khoản (taiKhoan)</span>
              <input
                value={settings.username}
                placeholder="Tài khoản đăng nhập API"
                onChange={(event) => onChange({ ...settings, username: event.target.value })}
                autoComplete="username"
              />
            </label>

            <label className="field">
              <span>Mật khẩu (matKhau)</span>
              <input
                type="password"
                value={settings.password}
                placeholder={
                  hasStoredPassword ? "Đã lưu — để trống nếu không đổi" : "Mật khẩu đăng nhập API"
                }
                onChange={(event) => onChange({ ...settings, password: event.target.value })}
                autoComplete="current-password"
              />
            </label>

            <label className="field">
              <span>
                Cơ sở khám bệnh <small className="field-hint">ID</small>
              </span>
              <input
                type="number"
                step={1}
                value={settings.dsCoSoKcbId}
                aria-label="ID cơ sở khám bệnh"
                title="ID cơ sở khám bệnh"
                onChange={(event) => {
                  const value = event.currentTarget.valueAsNumber;
                  if (Number.isInteger(value)) {
                    onChange({ ...settings, dsCoSoKcbId: value });
                  }
                }}
              />
            </label>

            <label className="field-checkbox form-grid--full">
              <input
                type="checkbox"
                checked={settings.copyRefractionToNewGlasses}
                onChange={(event) =>
                  onChange({
                    ...settings,
                    copyRefractionToNewGlasses: event.currentTarget.checked,
                  })
                }
              />
              <span>
                Sao chép kết quả khúc xạ sang kính mới
                <small>Mặc định tắt</small>
              </span>
            </label>
          </div>

          {saveError ? (
            <p className="settings-error" role="alert">
              {saveError}
            </p>
          ) : null}

          <div className="button-row">
            <button
              type="button"
              className="ds-button ds-button--ghost"
              onClick={onTestConnection}
              disabled={isTestingConnection || isSaving || isProcessing}
            >
              {isTestingConnection ? (
                <Loader2 size={16} strokeWidth={2} className="spin" aria-hidden="true" />
              ) : (
                <PlugZap size={16} strokeWidth={2} aria-hidden="true" />
              )}
              {isTestingConnection ? "Đang login…" : "Kiểm tra / Login HIS"}
            </button>
            <button
              type="button"
              className="ds-button ds-button--primary"
              onClick={onSave}
              disabled={isSaving || isTestingConnection || isProcessing}
            >
              {isSaving ? (
                <Loader2 size={16} strokeWidth={2} className="spin" aria-hidden="true" />
              ) : (
                <Save size={16} strokeWidth={2} aria-hidden="true" />
              )}
              {isSaving ? "Đang lưu…" : "Lưu & Login"}
            </button>
          </div>
        </div>

        <div className="form-panel ds-card">
          <div className="panel-heading">
            <div>
              <h2>Xử lý tự động KR-800</h2>
            </div>
          </div>

          <div
            className={`settings-switch${autoProcessEnabled ? " is-on" : ""}`}
            role="group"
            aria-labelledby="auto-process-label"
          >
            <div className="settings-switch__text">
              <span id="auto-process-label">Tự động xử lý KR-800</span>
              <small>
                {autoProcessEnabled
                  ? trackingFolder
                    ? "Đang BẬT — file waiting sẽ tự gửi HIS khi có file mới"
                    : "Đang BẬT — chưa có thư mục tracking (vào tab KR-800 để chọn)"
                  : "Đang TẮT — xử lý thủ công bằng nút «Xử lý» trên tab KR-800"}
              </small>
            </div>
            <button
              type="button"
              className="settings-switch__control"
              role="switch"
              aria-checked={autoProcessEnabled}
              aria-labelledby="auto-process-label"
              disabled={isTogglingAutoProcess || isSaving || isTestingConnection}
              onClick={() => onAutoProcessChange?.(!autoProcessEnabled)}
              title={autoProcessEnabled ? "Tắt tự động xử lý" : "Bật tự động xử lý"}
            >
              <span className="settings-switch__track" aria-hidden="true">
                <span className="settings-switch__thumb" />
              </span>
              <span className="settings-switch__state" aria-hidden="true">
                {isTogglingAutoProcess ? "…" : autoProcessEnabled ? "Bật" : "Tắt"}
              </span>
            </button>
          </div>

          {autoProcessStatus ? <p className="kr800-message">{autoProcessStatus}</p> : null}
          {autoProcessError ? (
            <p className="settings-error" role="alert">
              {autoProcessError}
            </p>
          ) : null}
        </div>

        <div className="form-panel ds-card">
          <div className="panel-heading">
            <div>
              <h2>Khởi động cùng Windows</h2>
            </div>
          </div>

          <div
            className={`settings-switch${autostartEnabled ? " is-on" : ""}`}
            role="group"
            aria-labelledby="autostart-label"
          >
            <div className="settings-switch__text">
              <span id="autostart-label">Chạy khi khởi động Windows</span>
              <small>
                {autostartEnabled
                  ? "Đang BẬT — app sẽ tự mở sau khi đăng nhập Windows"
                  : "Đang TẮT — cần mở app thủ công"}
              </small>
            </div>
            <button
              type="button"
              className="settings-switch__control"
              role="switch"
              aria-checked={autostartEnabled}
              aria-labelledby="autostart-label"
              disabled={isTogglingAutostart || isSaving || isTestingConnection}
              onClick={() => onAutostartChange?.(!autostartEnabled)}
              title={
                autostartEnabled
                  ? "Tắt khởi động cùng Windows"
                  : "Bật khởi động cùng Windows"
              }
            >
              <span className="settings-switch__track" aria-hidden="true">
                <span className="settings-switch__thumb" />
              </span>
              <span className="settings-switch__state" aria-hidden="true">
                {isTogglingAutostart ? "…" : autostartEnabled ? "Bật" : "Tắt"}
              </span>
            </button>
          </div>

          {autostartStatus ? <p className="kr800-message">{autostartStatus}</p> : null}
          {autostartError ? (
            <p className="settings-error" role="alert">
              {autostartError}
            </p>
          ) : null}
        </div>

        <div className="form-panel ds-card">
          <div className="panel-heading">
            <div>
              <h2>Nhật ký ứng dụng</h2>
              <p className="panel-lead">
                Request/response login (không gồm mật khẩu/token đầy đủ) được ghi log để debug trên
                mạng nội bộ.
              </p>
            </div>
            <div className="panel-actions">
              <button
                type="button"
                className="ds-button ds-button--primary"
                onClick={onExportLogs}
                disabled={isExportingLogs}
              >
                <Download size={16} strokeWidth={2} aria-hidden="true" />
                {isExportingLogs ? "Đang xuất…" : "Xuất logs"}
              </button>
            </div>
          </div>

          <div className="log-meta-grid">
            <div>
              <span>File log</span>
              <strong title={logInfo?.logPath}>{logInfo?.logPath || "—"}</strong>
            </div>
            <div>
              <span>Kích thước</span>
              <strong>{formatBytes(logInfo?.sizeBytes)}</strong>
            </div>
            <div>
              <span>Backup rotate</span>
              <strong>{logInfo?.hasBackup ? "Có (app.log.1)" : "Không"}</strong>
            </div>
          </div>

          {logStatus ? <p className="kr800-message">{logStatus}</p> : null}
          {logError ? (
            <p className="settings-error" role="alert">
              {logError}
            </p>
          ) : null}
        </div>
      </div>

      <aside className="settings-side-stack">
        <div className="form-panel ds-card settings-session-card">
          <div className="panel-heading">
            <div>
              <h2>Phiên đăng nhập HIS</h2>
              <p className="panel-lead">
                Token lưu bảng <code>auth_session</code> (không hiển thị full access_token trên UI).
              </p>
            </div>
          </div>

          <div className="log-meta-grid log-meta-grid--stack">
            <div>
              <span>Trạng thái</span>
              <strong>
                {hisAuth?.loggedIn
                  ? "Đã login"
                  : hisAuthError
                    ? "Lỗi"
                    : "Chưa login"}
              </strong>
            </div>
            <div>
              <span>User</span>
              <strong title={hisAuth?.fullName || hisAuth?.username || undefined}>
                {hisAuth?.fullName || hisAuth?.username || "—"}
              </strong>
            </div>
            <div>
              <span>coSoKcbId</span>
              <strong>{hisAuth?.coSoKcbId ?? "—"}</strong>
            </div>
            <div>
              <span>Hết hạn token</span>
              <strong title={hisAuth?.expiration || undefined}>
                {hisAuth?.expiration || "—"}
              </strong>
            </div>
            <div>
              <span>Token type</span>
              <strong>{hisAuth?.tokenType || "—"}</strong>
            </div>
            <div>
              <span>Có access_token</span>
              <strong>{hisAuth?.hasAccessToken ? "Có" : "Không"}</strong>
            </div>
          </div>

          {hisAuthError ? (
            <p className="settings-error" role="alert">
              {hisAuthError}
            </p>
          ) : null}
        </div>

        <div className="side-note">
          <div className="side-note__icon" aria-hidden="true">
            <Info size={16} strokeWidth={2} />
          </div>
          <span>Trạng thái</span>
          <strong>{connectionLabel}</strong>
          {settings.updatedAt ? (
            <p>
              Lần lưu cấu hình: <code>{formatUpdatedAt(settings.updatedAt)}</code>
            </p>
          ) : (
            <p>Chưa lưu cấu hình HIS trong SQLite.</p>
          )}
          <p>
            Endpoint: <code>{"{base}"}/api/his/v1/auth/login</code>
          </p>
        </div>
      </aside>
    </section>
  );
}

function formatUpdatedAt(value: string) {
  const normalized = value.includes("T") ? value : value.replace(" ", "T") + "Z";
  const date = new Date(normalized);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString("vi-VN", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function formatBytes(bytes?: number) {
  if (bytes == null || Number.isNaN(bytes)) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}
