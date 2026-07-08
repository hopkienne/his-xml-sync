import { useEffect, useState } from "react";
import type { AppSession } from "../App";
import { Sidebar } from "../components/Sidebar";
import { DashboardPanel } from "../features/dashboard/DashboardPanel";
import { LicensePanel } from "../features/license/LicensePanel";
import { LogsPanel } from "../features/logs/LogsPanel";
import { HisSettingsPanel } from "../features/settings/HisSettingsPanel";
import { SyncPanel } from "../features/sync/SyncPanel";
import { XmlFolderPanel } from "../features/xml-folder/XmlFolderPanel";
import { fallbackSettings, getSettings, runSyncOnce, saveSettings } from "../lib/appCommands";
import type {
  AppSettings,
  HomeMenuItem,
  LogEntry,
  LogStatus,
  MenuItemKey,
  SyncFileRow,
  SyncStat,
  SyncSummary,
} from "../types";

type HomeShellProps = {
  session: AppSession;
  onLogout: () => void;
};

const menuItems: HomeMenuItem[] = [
  { key: "dashboard", label: "Tổng quan", description: "Trạng thái đồng bộ và cảnh báo gần nhất" },
  { key: "his-settings", label: "Cấu hình HIS", description: "API, tài khoản và cơ sở KCB" },
  { key: "xml-folder", label: "Thư mục XML", description: "Nguồn file máy đo khúc xạ xuất ra" },
  { key: "sync", label: "Đồng bộ", description: "Xử lý XML và gửi kết quả lên HIS" },
  { key: "logs", label: "Nhật ký", description: "Lịch sử thành công, lỗi và file bị bỏ qua" },
  { key: "license", label: "License", description: "Khách hàng và ngày hết hạn" },
];

const stats: SyncStat[] = [
  { label: "File chờ xử lý", value: "12", tone: "neutral", description: "Trong thư mục XML" },
  { label: "Đã gửi hôm nay", value: "8", tone: "good", description: "Ghi nhận HIS thành công" },
  { label: "Cần kiểm tra", value: "3", tone: "warning", description: "Thiếu match người bệnh" },
  { label: "Lỗi", value: "1", tone: "danger", description: "Gửi API thất bại" },
];

const syncRows: SyncFileRow[] = [
  {
    id: "1",
    fileName: "HCM2607070269_20260707_145000_TOPCON_KR-800_4780634.xml",
    patientId: "HCM2607070269",
    measuredAt: "07/07/2026 14:50",
    status: "sent",
    right: { sphere: "+1.75", cylinder: "-1.00", axis: "178" },
    left: { sphere: "+0.75", cylinder: "-0.25", axis: "35" },
  },
  {
    id: "2",
    fileName: "HCM2607070312_20260707_151205_TOPCON_KR-800.xml",
    patientId: "HCM2607070312",
    measuredAt: "07/07/2026 15:12",
    status: "needs-review",
    error: "Chưa tìm thấy người bệnh trong danh sách HIS",
    right: { sphere: "-0.50", cylinder: "-0.75", axis: "92" },
    left: { sphere: "-0.25", cylinder: "-0.50", axis: "88" },
  },
  {
    id: "3",
    fileName: "HCM2607070345_20260707_153030_TOPCON_KR-800.xml",
    patientId: "HCM2607070345",
    measuredAt: "07/07/2026 15:30",
    status: "failed",
    error: "API HIS trả lỗi 500",
    right: { sphere: "+0.25", cylinder: "-0.25", axis: "12" },
    left: { sphere: "+0.50", cylinder: "-0.25", axis: "16" },
  },
  {
    id: "4",
    fileName: "HCM2607070366_20260707_160100_TOPCON_KR-800.xml",
    patientId: "HCM2607070366",
    measuredAt: "07/07/2026 16:01",
    status: "waiting",
    right: { sphere: "+1.00", cylinder: "-0.50", axis: "60" },
    left: { sphere: "+0.75", cylinder: "-0.75", axis: "102" },
  },
];

const logEntries: LogEntry[] = [
  {
    id: "log-1",
    time: "21:52",
    status: "success",
    message: "Đã gửi kết quả HCM2607070269",
    detail: "Payload khúc xạ đã được HIS ghi nhận.",
  },
  {
    id: "log-2",
    time: "21:48",
    status: "warning",
    message: "Không match được người bệnh HCM2607070312",
    detail: "Cần kiểm tra ngày vào viện hoặc trạng thái dịch vụ.",
  },
  {
    id: "log-3",
    time: "21:44",
    status: "error",
    message: "Gửi API thất bại",
    detail: "Endpoint nb-kham-ck-mat trả lỗi 500.",
  },
];

export function HomeShell({ session, onLogout }: HomeShellProps) {
  const [activeMenu, setActiveMenu] = useState<MenuItemKey>("dashboard");
  const [settings, setSettings] = useState<AppSettings>(fallbackSettings);
  const [connectionLabel, setConnectionLabel] = useState("Chưa kiểm tra");
  const [hisConnection, setHisConnection] = useState<"connected" | "idle">("idle");
  const [logFilter, setLogFilter] = useState<LogStatus | "all">("all");
  const [syncSummary, setSyncSummary] = useState<SyncSummary | undefined>();
  const [isSyncing, setIsSyncing] = useState(false);
  const currentMenu = menuItems.find((item) => item.key === activeMenu) ?? menuItems[0];

  useEffect(() => {
    let cancelled = false;

    getSettings().then((loadedSettings) => {
      if (!cancelled) {
        setSettings(loadedSettings);
      }
    });

    return () => {
      cancelled = true;
    };
  }, []);

  async function handleSaveSettings() {
    const savedSettings = await saveSettings(settings);
    setSettings(savedSettings);
    setConnectionLabel("Đã lưu cấu hình");
  }

  function handleTestConnection() {
    setConnectionLabel("Kết nối HIS sẵn sàng");
    setHisConnection("connected");
  }

  async function handleRunSync() {
    setIsSyncing(true);
    const summary = await runSyncOnce();
    setSyncSummary(summary);
    setIsSyncing(false);
  }

  function renderPanel() {
    switch (activeMenu) {
      case "dashboard":
        return (
          <DashboardPanel
            stats={stats}
            settings={settings}
            licenseExpiresAt={session.expiresAt}
            hisConnection={hisConnection}
          />
        );
      case "his-settings":
        return (
          <HisSettingsPanel
            settings={settings}
            onChange={setSettings}
            onSave={handleSaveSettings}
            onTestConnection={handleTestConnection}
            connectionLabel={connectionLabel}
          />
        );
      case "xml-folder":
        return <XmlFolderPanel settings={settings} onChange={setSettings} />;
      case "sync":
        return (
          <SyncPanel
            rows={syncRows}
            summary={syncSummary}
            onRunSync={handleRunSync}
            isSyncing={isSyncing}
          />
        );
      case "logs":
        return <LogsPanel entries={logEntries} filter={logFilter} onFilterChange={setLogFilter} />;
      case "license":
        return <LicensePanel session={session} onLogout={onLogout} />;
      default:
        return null;
    }
  }

  return (
    <main className="app-shell">
      <Sidebar items={menuItems} activeKey={activeMenu} onSelect={setActiveMenu} />

      <section className="content-area">
        <header className="topbar">
          <div>
            <h1>{currentMenu.label}</h1>
            <p>{currentMenu.description}</p>
          </div>
          <div className="session-pill">
            <div className="session-pill__meta">
              <span>{session.facilityName ?? session.customerName ?? "Chưa gán tên"}</span>
              <strong>Hết hạn: {formatDate(session.expiresAt)}</strong>
            </div>
            <button type="button" className="ds-button ds-button--ghost" onClick={onLogout}>
              Đổi key
            </button>
          </div>
        </header>

        <section className="work-surface">{renderPanel()}</section>
      </section>
    </main>
  );
}

function formatDate(value?: string) {
  if (!value) return "N/A";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleDateString("vi-VN", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
  });
}
