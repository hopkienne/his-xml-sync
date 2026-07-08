import type { LogEntry, LogStatus } from "../../types";

type LogsPanelProps = {
  entries: LogEntry[];
  filter: LogStatus | "all";
  onFilterChange: (filter: LogStatus | "all") => void;
};

const filterItems: Array<{ value: LogStatus | "all"; label: string }> = [
  { value: "all", label: "Tất cả" },
  { value: "success", label: "Thành công" },
  { value: "warning", label: "Cần kiểm tra" },
  { value: "error", label: "Lỗi" },
];

export function LogsPanel({ entries, filter, onFilterChange }: LogsPanelProps) {
  const visibleEntries = filter === "all" ? entries : entries.filter((entry) => entry.status === filter);

  return (
    <section className="logs-layout">
      <div className="panel-heading">
        <div>
          <h2>Nhật ký xử lý</h2>
          <p>Theo dõi các lần đọc XML, match người bệnh và gửi dữ liệu lên HIS.</p>
        </div>
        <div className="segmented-control" role="group" aria-label="Lọc nhật ký">
          {filterItems.map((item) => (
            <button
              key={item.value}
              type="button"
              className={filter === item.value ? "active" : ""}
              onClick={() => onFilterChange(item.value)}
            >
              {item.label}
            </button>
          ))}
        </div>
      </div>

      <div className="log-list">
        {visibleEntries.map((entry) => (
          <article className={`log-entry ${entry.status}`} key={entry.id}>
            <time>{entry.time}</time>
            <div>
              <strong>{entry.message}</strong>
              <p>{entry.detail}</p>
            </div>
          </article>
        ))}
      </div>
    </section>
  );
}
