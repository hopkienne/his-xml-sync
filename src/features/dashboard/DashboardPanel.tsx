import {
  AlertCircle,
  DatabaseZap,
  FolderOpen,
  KeyRound,
  RefreshCw,
  Settings2,
  UploadCloud,
  Wifi,
} from "lucide-react";
import type { ComponentType } from "react";
import { StatCard } from "../../components/StatCard";
import type {
  AppSettings,
  ConnectionState,
  SyncStat,
  WorkspaceTabKey,
  XmlFolderState,
} from "../../types";

type DashboardPanelProps = {
  stats: SyncStat[];
  settings: AppSettings;
  xmlFolder: XmlFolderState;
  licenseExpiresAt?: string;
  hisConnection: ConnectionState;
  lastSyncAt?: string;
  onNavigateTab: (key: WorkspaceTabKey) => void;
  onOpenSettings: () => void;
  onRunSync: () => void;
  isSyncing: boolean;
};

export function DashboardPanel({
  stats,
  settings,
  xmlFolder,
  licenseExpiresAt,
  hisConnection,
  lastSyncAt,
  onNavigateTab,
  onOpenSettings,
  onRunSync,
  isSyncing,
}: DashboardPanelProps) {
  const configReady =
    Boolean(settings.hisApiUrl?.trim()) &&
    Boolean(settings.username?.trim());

  const statusItems: Array<{
    icon: ComponentType<{ size?: number; strokeWidth?: number }>;
    label: string;
    value: string;
    tone: "neutral" | "good" | "warning" | "danger";
  }> = [
    {
      icon: KeyRound,
      label: "License",
      value: licenseExpiresAt ? `Còn hạn đến ${formatDate(licenseExpiresAt)}` : "Chưa có hạn dùng",
      tone: licenseExpiresAt ? "good" : "warning",
    },
    {
      icon: Wifi,
      label: "Kết nối HIS",
      value: configReady
        ? connectionLabel(hisConnection)
        : "Chưa cấu hình API",
      tone: configReady
        ? hisConnection === "connected"
          ? "good"
          : hisConnection === "error"
            ? "danger"
            : hisConnection === "warning"
              ? "warning"
              : "neutral"
        : "warning",
    },
    {
      icon: FolderOpen,
      label: "Thư mục XML",
      value: xmlFolder.xmlFolder || "Chưa chọn thư mục",
      tone: xmlFolder.xmlFolder ? "good" : "warning",
    },
    {
      icon: DatabaseZap,
      label: "Lần đồng bộ gần nhất",
      value: lastSyncAt || "Chưa đồng bộ",
      tone: lastSyncAt ? "good" : "neutral",
    },
  ];

  return (
    <div className="panel-stack">
      <section className="dashboard-grid" aria-label="Thống kê đồng bộ">
        {stats.map((stat) => (
          <StatCard key={stat.label} {...stat} />
        ))}
      </section>

      <section className="status-board" aria-label="Trạng thái hệ thống">
        {statusItems.map((item) => {
          const Icon = item.icon;
          return (
            <article className="status-tile" key={item.label}>
              <div className={`status-tile__icon ${item.tone}`}>
                <Icon size={18} strokeWidth={2} aria-hidden="true" />
              </div>
              <div className="status-tile__body">
                <span>{item.label}</span>
                <strong title={item.value}>{item.value}</strong>
              </div>
            </article>
          );
        })}
      </section>

      <section className="quick-actions" aria-label="Thao tác nhanh">
        <div className="quick-actions__label">Thao tác nhanh</div>
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
        <button
          type="button"
          className="ds-button ds-button--ghost"
          onClick={onOpenSettings}
        >
          <Settings2 size={16} strokeWidth={2} aria-hidden="true" />
          Cấu hình
        </button>
        <button
          type="button"
          className="ds-button ds-button--ghost"
          onClick={() => onNavigateTab("xml-folder")}
        >
          <FolderOpen size={16} strokeWidth={2} aria-hidden="true" />
          Thư mục XML
        </button>
        <button
          type="button"
          className="ds-button ds-button--ghost"
          onClick={() => onNavigateTab("logs")}
        >
          <AlertCircle size={16} strokeWidth={2} aria-hidden="true" />
          Xem nhật ký
        </button>
      </section>
    </div>
  );
}

function connectionLabel(state: ConnectionState) {
  switch (state) {
    case "connected":
      return "Sẵn sàng";
    case "warning":
      return "Cảnh báo";
    case "error":
      return "Lỗi kết nối";
    default:
      return "Chưa kiểm tra";
  }
}

function formatDate(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleDateString("vi-VN", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
  });
}
