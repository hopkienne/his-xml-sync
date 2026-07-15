import { Check, Copy, Loader2, Users, X } from "lucide-react";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { getLastPatientList } from "../../lib/appCommands";
import type { PatientListSnapshot } from "../../types";

type PatientListDialogProps = {
  open: boolean;
  onClose: () => void;
};

type CopyState = "idle" | "copied" | "error";

export function PatientListDialog({ open, onClose }: PatientListDialogProps) {
  const titleId = useId();
  const [snapshot, setSnapshot] = useState<PatientListSnapshot | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copyState, setCopyState] = useState<CopyState>("idle");
  const copyResetTimer = useRef<number | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setIsLoading(true);
    setError(null);
    setCopyState("idle");
    void getLastPatientList()
      .then((data) => {
        if (cancelled) return;
        if (!data) {
          setSnapshot(null);
          setError("Chưa có dữ liệu danh sách bệnh nhân trong phiên này.");
          return;
        }
        setSnapshot(data);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setSnapshot(null);
        setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, onClose]);

  useEffect(() => {
    return () => {
      if (copyResetTimer.current != null) {
        window.clearTimeout(copyResetTimer.current);
      }
    };
  }, []);

  const prettyJson = useMemo(() => {
    if (!snapshot?.body) return "";
    try {
      return JSON.stringify(JSON.parse(snapshot.body), null, 2);
    } catch {
      return snapshot.body;
    }
  }, [snapshot]);

  async function handleCopyJson() {
    if (!prettyJson) return;
    try {
      await copyTextToClipboard(prettyJson);
      setCopyState("copied");
    } catch {
      setCopyState("error");
    }
    if (copyResetTimer.current != null) {
      window.clearTimeout(copyResetTimer.current);
    }
    copyResetTimer.current = window.setTimeout(() => {
      setCopyState("idle");
      copyResetTimer.current = null;
    }, 2000);
  }

  if (!open) return null;

  const canCopy = Boolean(prettyJson) && !isLoading && !error;
  const copyLabel =
    copyState === "copied"
      ? "Đã copy"
      : copyState === "error"
        ? "Copy thất bại"
        : "Copy JSON";

  return (
    <div
      className="params-modal-overlay"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        className="params-modal params-modal--wide ds-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
      >
        <div className="params-modal__header">
          <div>
            <h2 id={titleId}>
              <Users size={18} strokeWidth={2} aria-hidden="true" className="params-modal__title-icon" />
              Danh sách bệnh nhân
            </h2>
            <p className="params-modal__lead">
              JSON trả về từ API{" "}
              <code>/api/his/v1/nb-kham-ck-mat/nguoi-benh</code>
              {snapshot
                ? ` — ${snapshot.patientCount.toLocaleString("vi-VN")} bản ghi · ${snapshot.fromTime} → ${snapshot.toTime}`
                : null}
            </p>
          </div>
          <button
            type="button"
            className="ds-button ds-button--ghost params-modal__close"
            onClick={onClose}
            aria-label="Đóng"
            title="Đóng"
          >
            <X size={18} strokeWidth={2} aria-hidden="true" />
          </button>
        </div>

        {snapshot?.fetchedAt ? (
          <p className="params-modal__hint">
            Lần lấy dữ liệu gần nhất: <strong>{snapshot.fetchedAt}</strong>
          </p>
        ) : null}

        <div className="params-modal__body">
          {isLoading ? (
            <div className="params-modal__loading">
              <Loader2 size={18} strokeWidth={2} className="spin" aria-hidden="true" />
              Đang tải JSON…
            </div>
          ) : error ? (
            <p className="settings-error" role="alert">
              {error}
            </p>
          ) : (
            <div className="patient-json-panel">
              <div className="patient-json-panel__toolbar">
                <span className="patient-json-panel__label">Response JSON</span>
                <button
                  type="button"
                  className={`ds-button ds-button--ghost patient-json-copy${
                    copyState === "copied" ? " is-copied" : ""
                  }${copyState === "error" ? " is-error" : ""}`}
                  onClick={() => void handleCopyJson()}
                  disabled={!canCopy}
                  title="Copy toàn bộ JSON vào clipboard"
                >
                  {copyState === "copied" ? (
                    <Check size={15} strokeWidth={2.25} aria-hidden="true" />
                  ) : (
                    <Copy size={15} strokeWidth={2} aria-hidden="true" />
                  )}
                  {copyLabel}
                </button>
              </div>
              <pre className="patient-json-view" tabIndex={0}>
                {prettyJson}
              </pre>
            </div>
          )}
        </div>

        <div className="params-modal__footer">
          <div className="params-modal__footer-right">
            <button type="button" className="ds-button ds-button--primary" onClick={onClose}>
              Đóng
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

async function copyTextToClipboard(text: string): Promise<void> {
  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }
  // Fallback (một số môi trường WebView / quyền clipboard hạn chế)
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.left = "-9999px";
  area.style.top = "0";
  document.body.appendChild(area);
  area.select();
  const ok = document.execCommand("copy");
  document.body.removeChild(area);
  if (!ok) {
    throw new Error("execCommand copy failed");
  }
}
