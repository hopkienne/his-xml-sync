import { DatabaseZap, FolderCheck, KeyRound, Wifi } from "lucide-react";
import { StatCard } from "../../components/StatCard";
import type { AppSettings, SyncStat } from "../../types";

type DashboardPanelProps = {
  stats: SyncStat[];
  settings: AppSettings;
  licenseExpiresAt?: string;
  hisConnection: "connected" | "idle";
};

export function DashboardPanel({
  stats,
  settings,
  licenseExpiresAt,
  hisConnection,
}: DashboardPanelProps) {
  const statusItems = [
    {
      icon: KeyRound,
      label: "License",
      value: licenseExpiresAt ? `Còn hạn đến ${formatDate(licenseExpiresAt)}` : "Chưa có hạn dùng",
      tone: "good",
    },
    {
      icon: Wifi,
      label: "Kết nối HIS",
      value: hisConnection === "connected" ? "Sẵn sàng" : "Chưa kiểm tra",
      tone: hisConnection === "connected" ? "good" : "idle",
    },
    {
      icon: FolderCheck,
      label: "Thư mục XML",
      value: settings.xmlFolder || "Chưa chọn thư mục",
      tone: settings.xmlFolder ? "good" : "idle",
    },
    {
      icon: DatabaseZap,
      label: "Lần đồng bộ gần nhất",
      value: "08/07/2026 21:52",
      tone: "good",
    },
  ];

  return (
    <div className="panel-stack">
      <section className="dashboard-grid">
        {stats.map((stat) => (
          <StatCard key={stat.label} {...stat} />
        ))}
      </section>

      <section className="panel-grid panel-grid--status">
        {statusItems.map((item) => {
          const Icon = item.icon;
          return (
            <article className="status-tile" key={item.label}>
              <div className={`status-tile__icon ${item.tone}`}>
                <Icon size={18} strokeWidth={2} aria-hidden="true" />
              </div>
              <div>
                <span>{item.label}</span>
                <strong>{item.value}</strong>
              </div>
            </article>
          );
        })}
      </section>
    </div>
  );
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
