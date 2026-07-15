import { Loader2, Plus, RotateCcw, Save, Trash2, X } from "lucide-react";
import { useEffect, useId, useState } from "react";
import {
  defaultPatientQueryParams,
  getPatientQueryParams,
  isDefaultPatientParamKey,
  isProcessRangeBoundParam,
  savePatientQueryParams,
} from "../../lib/appCommands";
import type { PatientQueryParam } from "../../types";

type ProcessRangeProps = {
  /** datetime-local: YYYY-MM-DDTHH:mm */
  from: string;
  to: string;
};

type PatientParamsDialogProps = {
  open: boolean;
  onClose: () => void;
  /** Khoảng «Ngày xử lý» hiện tại — đổ vào tu/denThoiGianVaoVien. */
  processRange: ProcessRangeProps;
};

type DraftRow = PatientQueryParam & {
  id: string;
  enabled: boolean;
  /** true = param người dùng thêm → hiện thùng rác. */
  removable: boolean;
};

export function PatientParamsDialog({
  open,
  onClose,
  processRange,
}: PatientParamsDialogProps) {
  const titleId = useId();
  const [rows, setRows] = useState<DraftRow[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const rangeFromApi = toHisApiDateTime(processRange.from);
  const rangeToApi = toFilterEndDateTime(processRange.to);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setIsLoading(true);
    setError(null);
    void getPatientQueryParams()
      .then((params) => {
        if (cancelled) return;
        setRows(toDraftRows(params, processRange));
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : String(err));
        setRows(toDraftRows(defaultPatientQueryParams(), processRange));
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // Chỉ load lại khi mở popup; range sync riêng qua effect dưới.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // Đồng bộ value tu/den theo datetime picker khi range đổi (popup đang mở).
  useEffect(() => {
    if (!open) return;
    setRows((prev) =>
      prev.map((row) => {
        if (row.key.trim() === "tuThoiGianVaoVien") {
          return { ...row, value: rangeFromApi };
        }
        if (row.key.trim() === "denThoiGianVaoVien") {
          return { ...row, value: rangeToApi };
        }
        return row;
      }),
    );
  }, [open, rangeFromApi, rangeToApi]);

  useEffect(() => {
    if (!open) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !isSaving) {
        onClose();
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, isSaving, onClose]);

  if (!open) return null;

  function updateRow(id: string, patch: Partial<DraftRow>) {
    setRows((prev) =>
      prev.map((row) => {
        if (row.id !== id) return row;
        const next = { ...row, ...patch };
        // Đổi key → nếu là tu/den thì đổ value từ range.
        if (patch.key !== undefined) {
          if (patch.key.trim() === "tuThoiGianVaoVien") {
            next.value = rangeFromApi;
          } else if (patch.key.trim() === "denThoiGianVaoVien") {
            next.value = rangeToApi;
          }
        }
        return next;
      }),
    );
  }

  function removeRow(id: string) {
    setRows((prev) => prev.filter((row) => row.id !== id));
  }

  function addRow() {
    setRows((prev) => [
      ...prev,
      { id: newRowId(), key: "", value: "", enabled: true, removable: true },
    ]);
  }

  function resetDefaults() {
    setRows(toDraftRows(defaultPatientQueryParams(), processRange));
    setError(null);
  }

  function toggleAll(enabled: boolean) {
    setRows((prev) => prev.map((row) => ({ ...row, enabled })));
  }

  async function handleSave() {
    const cleaned = rows.map((row) => ({
      key: row.key.trim(),
      // Không persist value thời gian tĩnh — runtime luôn lấy từ «Ngày xử lý».
      value: isProcessRangeBoundParam(row.key) ? "" : row.value,
      enabled: row.enabled,
    }));
    const emptyKey = cleaned.find((row) => !row.key);
    if (emptyKey) {
      setError("Tên tham số (key) không được để trống.");
      return;
    }
    const keys = cleaned.map((row) => row.key);
    const dup = keys.find((key, index) => keys.indexOf(key) !== index);
    if (dup) {
      setError(`Tham số «${dup}» bị trùng.`);
      return;
    }

    setIsSaving(true);
    setError(null);
    try {
      const saved = await savePatientQueryParams(cleaned);
      setRows(toDraftRows(saved, processRange));
      onClose();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setIsSaving(false);
    }
  }

  const enabledCount = rows.filter((row) => row.enabled).length;

  return (
    <div
      className="params-modal-overlay"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !isSaving) onClose();
      }}
    >
      <div
        className="params-modal ds-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
      >
        <div className="params-modal__header">
          <div>
            <h2 id={titleId}>Tham số API người bệnh</h2>
            <p className="params-modal__lead">
              Query params khi gọi{" "}
              <code>/api/his/v1/nb-kham-ck-mat/nguoi-benh</code>. Chỉ các dòng
              được tick sẽ gửi lên API ({enabledCount}/{rows.length}).
            </p>
          </div>
          <button
            type="button"
            className="ds-button ds-button--ghost params-modal__close"
            onClick={onClose}
            disabled={isSaving}
            aria-label="Đóng"
            title="Đóng"
          >
            <X size={18} strokeWidth={2} aria-hidden="true" />
          </button>
        </div>

        <p className="params-modal__hint">
          <code>tuThoiGianVaoVien</code> / <code>denThoiGianVaoVien</code> luôn
          lấy theo «Ngày xử lý» hiện tại (
          <strong>{rangeFromApi || "—"}</strong> → <strong>{rangeToApi || "—"}</strong>
          ). Bỏ tick để không gửi tham số đó.
        </p>

        <div className="params-modal__body">
          {isLoading ? (
            <div className="params-modal__loading">
              <Loader2 size={18} strokeWidth={2} className="spin" aria-hidden="true" />
              Đang tải tham số…
            </div>
          ) : (
            <div className="params-list" role="table" aria-label="Danh sách tham số">
              <div className="params-list__head" role="row">
                <div className="params-list__cell params-list__cell--check" role="columnheader">
                  <span className="visually-hidden">Gửi</span>
                  <input
                    type="checkbox"
                    className="params-table__checkbox"
                    checked={rows.length > 0 && rows.every((row) => row.enabled)}
                    ref={(el) => {
                      if (el) {
                        el.indeterminate =
                          rows.some((row) => row.enabled) &&
                          !rows.every((row) => row.enabled);
                      }
                    }}
                    onChange={(event) => toggleAll(event.target.checked)}
                    title="Chọn / bỏ chọn tất cả"
                    aria-label="Chọn tất cả tham số để gửi"
                    disabled={rows.length === 0}
                  />
                </div>
                <div className="params-list__cell params-list__cell--key" role="columnheader">
                  Key
                </div>
                <div className="params-list__cell params-list__cell--value" role="columnheader">
                  Value
                </div>
                <div className="params-list__cell params-list__cell--action" role="columnheader">
                  <span className="visually-hidden">Xoá</span>
                </div>
              </div>

              {rows.length === 0 ? (
                <div className="params-list__empty">
                  Chưa có tham số. Bấm «Thêm» hoặc «Mặc định».
                </div>
              ) : (
                rows.map((row) => {
                  const rangeBound = isProcessRangeBoundParam(row.key);
                  return (
                    <div
                      key={row.id}
                      className={`params-list__row${row.enabled ? "" : " is-off"}`}
                      role="row"
                    >
                      <div className="params-list__cell params-list__cell--check" role="cell">
                        <input
                          type="checkbox"
                          className="params-table__checkbox"
                          checked={row.enabled}
                          onChange={(event) =>
                            updateRow(row.id, { enabled: event.target.checked })
                          }
                          title={
                            row.enabled
                              ? "Đang gửi tham số này"
                              : "Không gửi tham số này"
                          }
                          aria-label={`Gửi ${row.key || "tham số"}`}
                        />
                      </div>
                      <div className="params-list__cell params-list__cell--key" role="cell">
                        <input
                          className="params-table__input"
                          value={row.key}
                          spellCheck={false}
                          autoComplete="off"
                          placeholder="tên tham số"
                          aria-label="Key"
                          onChange={(event) =>
                            updateRow(row.id, { key: event.target.value })
                          }
                        />
                      </div>
                      <div className="params-list__cell params-list__cell--value" role="cell">
                        <input
                          className="params-table__input"
                          value={row.value}
                          spellCheck={false}
                          autoComplete="off"
                          placeholder={rangeBound ? "Theo «Ngày xử lý»" : "giá trị"}
                          readOnly={rangeBound}
                          title={
                            rangeBound
                              ? "Giá trị lấy tự động từ 2 datetime picker «Ngày xử lý»"
                              : undefined
                          }
                          aria-label={`Value cho ${row.key || "tham số"}`}
                          onChange={(event) => {
                            if (rangeBound) return;
                            updateRow(row.id, { value: event.target.value });
                          }}
                        />
                      </div>
                      <div className="params-list__cell params-list__cell--action" role="cell">
                        {row.removable ? (
                          <button
                            type="button"
                            className="params-table__delete"
                            onClick={() => removeRow(row.id)}
                            title={`Xoá ${row.key || "tham số"}`}
                            aria-label={`Xoá ${row.key || "tham số"}`}
                          >
                            <Trash2 size={16} strokeWidth={2.25} aria-hidden="true" />
                          </button>
                        ) : null}
                      </div>
                    </div>
                  );
                })
              )}
            </div>
          )}
        </div>

        {error ? (
          <p className="settings-error" role="alert">
            {error}
          </p>
        ) : null}

        <div className="params-modal__footer">
          <div className="params-modal__footer-left">
            <button
              type="button"
              className="ds-button ds-button--ghost"
              onClick={addRow}
              disabled={isLoading || isSaving}
            >
              <Plus size={16} strokeWidth={2} aria-hidden="true" />
              Thêm
            </button>
            <button
              type="button"
              className="ds-button ds-button--ghost"
              onClick={resetDefaults}
              disabled={isLoading || isSaving}
              title="Khôi phục bộ tham số mặc định"
            >
              <RotateCcw size={16} strokeWidth={2} aria-hidden="true" />
              Mặc định
            </button>
          </div>
          <div className="params-modal__footer-right">
            <button
              type="button"
              className="ds-button ds-button--ghost"
              onClick={onClose}
              disabled={isSaving}
            >
              Huỷ
            </button>
            <button
              type="button"
              className="ds-button ds-button--primary"
              onClick={() => void handleSave()}
              disabled={isLoading || isSaving}
            >
              {isSaving ? (
                <Loader2 size={16} strokeWidth={2} className="spin" aria-hidden="true" />
              ) : (
                <Save size={16} strokeWidth={2} aria-hidden="true" />
              )}
              {isSaving ? "Đang lưu…" : "Lưu"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function toDraftRows(
  params: PatientQueryParam[],
  processRange: ProcessRangeProps,
): DraftRow[] {
  const from = toHisApiDateTime(processRange.from);
  const to = toFilterEndDateTime(processRange.to);
  return params.map((item) => {
    const key = item.key;
    let value = item.value;
    if (key.trim() === "tuThoiGianVaoVien") value = from;
    else if (key.trim() === "denThoiGianVaoVien") value = to;
    return {
      id: newRowId(),
      key,
      value,
      enabled: item.enabled !== false,
      // Param mặc định: không xoá. Param custom (vd. đã Lưu «dd»): có thùng rác.
      removable: !isDefaultPatientParamKey(key),
    };
  });
}

/** Format HIS API: `YYYY-MM-DD HH:mm:ss` (giây mặc định 00). */
function toHisApiDateTime(value: string): string {
  return normalizeDateTimeBound(value, "start");
}

/** Mốc cuối khoảng — nếu không có giây thì lấy hết phút đó (`:59`). */
function toFilterEndDateTime(value: string): string {
  return normalizeDateTimeBound(value, "end");
}

function normalizeDateTimeBound(value: string, bound: "start" | "end"): string {
  if (!value) return "";
  const normalized = value.includes("T") ? value : value.replace(" ", "T");
  const [date, time = "00:00"] = normalized.split("T");
  const timeParts = time.split(":");
  const hh = timeParts[0] ?? "00";
  const mm = timeParts[1] ?? "00";
  const hasSeconds = timeParts.length >= 3 && timeParts[2] !== "";
  const ss = hasSeconds ? (timeParts[2] ?? "00") : bound === "end" ? "59" : "00";
  return `${date} ${hh.padStart(2, "0")}:${mm.padStart(2, "0")}:${String(ss).padStart(2, "0")}`;
}

function newRowId(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 9)}`;
}
