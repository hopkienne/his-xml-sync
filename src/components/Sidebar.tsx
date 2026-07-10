import { ChevronLeft, ChevronRight, Settings2 } from "lucide-react";
import { useState } from "react";
import type { SidebarNavItem, SidebarNavKey } from "../types";

const STORAGE_KEY = "his-xml-sync.sidebar-collapsed";

type SidebarProps = {
  items: SidebarNavItem[];
  activeKey: SidebarNavKey;
  onSelect: (key: SidebarNavKey) => void;
  facilityLabel?: string;
};

function readCollapsedPreference(): boolean {
  try {
    return window.localStorage.getItem(STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

export function Sidebar({ items, activeKey, onSelect, facilityLabel }: SidebarProps) {
  const [collapsed, setCollapsed] = useState(readCollapsedPreference);

  const deviceItems = items.filter((item) => item.section === "device");
  const systemItems = items.filter((item) => item.section === "system");

  function toggleCollapsed() {
    setCollapsed((prev) => {
      const next = !prev;
      try {
        window.localStorage.setItem(STORAGE_KEY, next ? "1" : "0");
      } catch {
        /* ignore quota / private mode */
      }
      return next;
    });
  }

  return (
    <aside
      className={`sidebar${collapsed ? " is-collapsed" : ""}`}
      aria-label="Thanh điều hướng"
      data-collapsed={collapsed ? "true" : "false"}
    >
      <div className="sidebar__top">
        <div className="brand-block" title="HIS XML Sync">
          <div className="brand-mark" aria-hidden="true">
            HX
          </div>
          {!collapsed ? (
            <div className="brand-block__text">
              <div className="brand-block__title">HIS XML Sync</div>
              <div className="brand-block__subtitle">Đồng bộ máy đo</div>
            </div>
          ) : null}
        </div>
      </div>

      <div className="sidebar-body">
        <SidebarSection
          label="Máy đo"
          collapsed={collapsed}
          items={deviceItems}
          activeKey={activeKey}
          onSelect={onSelect}
          variant="device"
        />

        <SidebarSection
          label="Hệ thống"
          collapsed={collapsed}
          items={systemItems}
          activeKey={activeKey}
          onSelect={onSelect}
          variant="system"
        />
      </div>

      <div className="sidebar-footer" title={facilityLabel || "Chưa gán tên"}>
        {!collapsed ? (
          <>
            <div className="sidebar-footer__label">Cơ sở</div>
            <div className="sidebar-footer__value">{facilityLabel || "Chưa gán tên"}</div>
          </>
        ) : (
          <div className="sidebar-footer__dot" aria-hidden="true" />
        )}
      </div>

      <button
        type="button"
        className="sidebar-toggle"
        onClick={toggleCollapsed}
        aria-label={collapsed ? "Mở rộng sidebar" : "Thu gọn sidebar"}
        aria-expanded={!collapsed}
        title={collapsed ? "Mở rộng sidebar" : "Thu gọn sidebar"}
      >
        {collapsed ? (
          <ChevronRight size={14} strokeWidth={2} aria-hidden="true" />
        ) : (
          <ChevronLeft size={14} strokeWidth={2} aria-hidden="true" />
        )}
      </button>
    </aside>
  );
}

type SidebarSectionProps = {
  label: string;
  collapsed: boolean;
  items: SidebarNavItem[];
  activeKey: SidebarNavKey;
  onSelect: (key: SidebarNavKey) => void;
  variant: "device" | "system";
};

function SidebarSection({
  label,
  collapsed,
  items,
  activeKey,
  onSelect,
  variant,
}: SidebarSectionProps) {
  if (items.length === 0) return null;

  return (
    <div className="sidebar-section">
      {!collapsed ? <div className="sidebar-section__label">{label}</div> : null}
      <nav className="sidebar-nav" aria-label={label}>
        {items.map((item) => {
          const isActive = item.key === activeKey;
          return (
            <button
              key={item.key}
              type="button"
              className={isActive ? "active" : undefined}
              onClick={() => onSelect(item.key)}
              aria-current={isActive ? "page" : undefined}
              title={item.description}
            >
              {variant === "device" ? (
                <span className="device-glyph" aria-hidden="true">
                  {deviceShortLabel(item.label)}
                </span>
              ) : (
                <span className="nav-icon" aria-hidden="true">
                  <Settings2 size={15} strokeWidth={1.75} />
                </span>
              )}
              {!collapsed ? (
                <span className="device-meta">
                  <span className="device-meta__name">{item.label}</span>
                  {variant === "device" ? (
                    <span className="device-meta__hint">TOPCON · khúc xạ</span>
                  ) : (
                    <span className="device-meta__hint">API · SQLite</span>
                  )}
                </span>
              ) : null}
            </button>
          );
        })}
      </nav>
    </div>
  );
}

function deviceShortLabel(label: string) {
  const cleaned = label.replace(/\s+/g, "");
  if (cleaned.length <= 3) return cleaned;
  return cleaned.slice(0, 3);
}
