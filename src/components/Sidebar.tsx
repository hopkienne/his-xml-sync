import {
  Activity,
  ClipboardList,
  Folder,
  KeyRound,
  ScrollText,
  Settings,
} from "lucide-react";
import type { ComponentType } from "react";
import type { HomeMenuItem, MenuItemKey } from "../types";

type SidebarProps = {
  items: HomeMenuItem[];
  activeKey: MenuItemKey;
  onSelect: (key: MenuItemKey) => void;
};

const menuIcons: Record<MenuItemKey, ComponentType<{ size?: number; strokeWidth?: number }>> = {
  dashboard: Activity,
  "his-settings": Settings,
  "xml-folder": Folder,
  sync: ClipboardList,
  logs: ScrollText,
  license: KeyRound,
};

export function Sidebar({ items, activeKey, onSelect }: SidebarProps) {
  return (
    <aside className="sidebar">
      <div className="brand-block">
        <div className="brand-logo" aria-hidden="true">
          HX
        </div>
        <div>
          <div className="ds-kicker">HIS XML Sync</div>
          <span>TOPCON KR-800</span>
        </div>
      </div>

      <nav className="sidebar-nav" aria-label="Điều hướng chính">
        {items.map((item) => {
          const Icon = menuIcons[item.key];
          return (
            <button
              key={item.key}
              type="button"
              className={item.key === activeKey ? "active" : ""}
              onClick={() => onSelect(item.key)}
            >
              <Icon size={18} strokeWidth={2} aria-hidden="true" />
              <span>{item.label}</span>
              <small>{item.description}</small>
            </button>
          );
        })}
      </nav>
    </aside>
  );
}
