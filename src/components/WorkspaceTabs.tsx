import type { WorkspaceTab, WorkspaceTabKey } from "../types";

type WorkspaceTabsProps = {
  items: WorkspaceTab[];
  activeKey: WorkspaceTabKey;
  onSelect: (key: WorkspaceTabKey) => void;
};

export function WorkspaceTabs({ items, activeKey, onSelect }: WorkspaceTabsProps) {
  return (
    <nav className="workspace-tabs" aria-label="Chức năng máy đo">
      {items.map((item) => {
        const isActive = item.key === activeKey;
        return (
          <button
            key={item.key}
            type="button"
            className={isActive ? "active" : undefined}
            onClick={() => onSelect(item.key)}
            aria-current={isActive ? "page" : undefined}
          >
            {item.label}
          </button>
        );
      })}
    </nav>
  );
}
