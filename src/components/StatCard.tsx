import { AlertTriangle, CheckCircle2, Clock3, XCircle } from "lucide-react";
import type { ComponentType } from "react";
import type { SyncStat } from "../types";

type StatCardProps = SyncStat & {
  description?: string;
};

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
      <span>{label}</span>
      <strong>{value}</strong>
      {description ? <small>{description}</small> : null}
    </article>
  );
}
