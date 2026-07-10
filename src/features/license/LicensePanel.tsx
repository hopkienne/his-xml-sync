import { KeyRound, ShieldCheck } from "lucide-react";
import type { AppSession } from "../../App";

type LicensePanelProps = {
  session: AppSession;
  onLogout: () => void;
};

export function LicensePanel({ session, onLogout }: LicensePanelProps) {
  return (
    <section className="license-card ds-card">
      <div className="license-emblem" aria-hidden="true">
        <ShieldCheck size={28} strokeWidth={2} />
      </div>

      <div className="license-card__body">
        <div className="ds-kicker">License hợp lệ</div>
        <h2 title={session.facilityName ?? undefined}>
          {session.facilityName ?? "Cơ sở HIS demo"}
        </h2>

        <span className="license-status-pill">
          <ShieldCheck size={14} strokeWidth={2} aria-hidden="true" />
          Đang hoạt động
        </span>

        <dl className="license-details" style={{ marginTop: 16 }}>
          <div>
            <dt>Khách hàng</dt>
            <dd title={session.customerName}>{session.customerName ?? "Phòng khám demo"}</dd>
          </div>
          <div>
            <dt>Cơ sở sử dụng</dt>
            <dd title={session.facilityName}>{session.facilityName ?? "—"}</dd>
          </div>
          <div>
            <dt>Ngày hết hạn</dt>
            <dd>{formatDate(session.expiresAt)}</dd>
          </div>
          <div>
            <dt>Trạng thái</dt>
            <dd>Valid</dd>
          </div>
        </dl>
      </div>

      <button type="button" className="ds-button ds-button--ghost" onClick={onLogout}>
        <KeyRound size={16} strokeWidth={2} aria-hidden="true" />
        Đổi key
      </button>
    </section>
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
