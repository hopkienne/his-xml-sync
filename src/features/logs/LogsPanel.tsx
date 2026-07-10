import type { LogEntry, LogStatus } from "../../types";

type LogsPanelProps = {
  entries: LogEntry[];
  filter: LogStatus | "all";
  dateFilter: string;
  onFilterChange: (filter: LogStatus | "all") => void;
  onDateFilterChange: (date: string) => void;
};

const filterItems: Array<{ value: LogStatus | "all"; label: string }> = [
  { value: "all", label: "Tất cả" },
  { value: "success", label: "Thành công" },
  { value: "warning", label: "Cần kiểm tra" },
  { value: "error", label: "Lỗi" },
];

export function LogsPanel({
  entries,
  filter,
  dateFilter,
  onFilterChange,
  onDateFilterChange,
}: LogsPanelProps) {
  const visibleEntries = entries.filter((entry) => {
    const matchStatus = filter === "all" || entry.status === filter;
    const matchDate = !dateFilter || entry.date === dateFilter;
    return matchStatus && matchDate;
  });

  return (
    <section className="logs-layout">
      <div className="logs-toolbar">
        <div>
          <h2 className="panel-section-title">Nhật ký xử lý</h2>
          <p className="panel-lead">
            Theo dõi lần đọc XML, match người bệnh và gửi dữ liệu lên HIS.
          </p>
        </div>

        <div className="logs-filters">
          <label className="field">
            <span>Theo ngày</span>
            <input
              type="date"
              value={dateFilter}
              onChange={(event) => onDateFilterChange(event.target.value)}
            />
          </label>

          <div className="segmented-control" role="group" aria-label="Lọc theo trạng thái">
            {filterItems.map((item) => (
              <button
                key={item.value}
                type="button"
                className={filter === item.value ? "active" : undefined}
                onClick={() => onFilterChange(item.value)}
              >
                {item.label}
              </button>
            ))}
          </div>
        </div>
      </div>

      {visibleEntries.length === 0 ? (
        <div className="log-empty">Không có bản ghi khớp bộ lọc.</div>
      ) : (
        <div className="log-list" role="list">
          {visibleEntries.map((entry) => (
            <article className={`log-entry ${entry.status}`} key={entry.id} role="listitem">
              <div className="log-entry__when">
                <strong>{entry.time}</strong>
                <span>{formatDisplayDate(entry.date)}</span>
              </div>
              <div className="log-entry__body">
                <strong title={entry.message}>{entry.message}</strong>
                <p title={entry.detail}>{entry.detail}</p>
              </div>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

function formatDisplayDate(isoDate: string) {
  const [year, month, day] = isoDate.split("-");
  if (!year || !month || !day) return isoDate;
  return `${day}/${month}/${year}`;
}
