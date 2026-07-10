import { AlertTriangle, CheckCircle2, Clock3, XCircle } from "lucide-react";
import type { ComponentType } from "react";
import type { SyncStat } from "../types";

type StatCardProps = SyncStat;

const toneIcons: Record<SyncStat["tone"], ComponentType<{ size?: number; strokeWidth?: number }>> = {
  neutral: Clock3,
  good: CheckCircle2,
  warning: AlertTriangle,
  danger: XCircle,
};

export function StatCard({ label, value, tone, description }: StatCardProps) {
  const Icon = toneIcons[tone];

  return (
    <article className={`stat-card ${tone}`}>
      <div className="stat-card__icon" aria-hidden="true">
        <Icon size={18} strokeWidth={2} />
      </div>
      <span title={label}>{label}</span>
      <strong title={value}>{value}</strong>
      {description ? <small title={description}>{description}</small> : null}
    </article>
  );
}
