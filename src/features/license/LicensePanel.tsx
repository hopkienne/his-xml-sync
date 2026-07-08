import { KeyRound, ShieldCheck } from "lucide-react";
import type { AppSession } from "../../App";

type LicensePanelProps = {
  session: AppSession;
  onLogout: () => void;
};

export function LicensePanel({ session, onLogout }: LicensePanelProps) {
  return (
    <section className="license-layout ds-card">
      <div className="license-emblem">
        <ShieldCheck size={28} strokeWidth={2} aria-hidden="true" />
      </div>
      <div>
        <div className="ds-kicker">License hợp lệ</div>
        <h2>{session.facilityName ?? "Cơ sở HIS demo"}</h2>
        <dl className="license-details">
          <div>
            <dt>Khách hàng</dt>
            <dd>{session.customerName ?? "Phòng khám demo"}</dd>
          </div>
          <div>
            <dt>Hạn sử dụng</dt>
            <dd>{session.expiresAt ?? "N/A"}</dd>
          </div>
          <div>
            <dt>Trạng thái</dt>
            <dd>Đang hoạt động</dd>
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
