export type MenuItemKey =
  | "dashboard"
  | "his-settings"
  | "xml-folder"
  | "sync"
  | "logs"
  | "license";

export type HomeMenuItem = {
  key: MenuItemKey;
  label: string;
  description: string;
};

export type SyncStat = {
  label: string;
  value: string;
  tone: "neutral" | "good" | "warning" | "danger";
  description?: string;
};

export type ConnectionState = "connected" | "warning" | "error" | "idle";

export type AppSettings = {
  apiBaseUrl: string;
  username: string;
  xmlFolder?: string | null;
  facilityId?: number | null;
  autoSyncEnabled: boolean;
};

export type SyncSummary = {
  scannedFiles: number;
  sentResults: number;
  skippedFiles: number;
  failedFiles: number;
};

export type SyncFileStatus = "waiting" | "sent" | "needs-review" | "failed";

export type SyncFileRow = {
  id: string;
  fileName: string;
  patientId: string;
  measuredAt: string;
  status: SyncFileStatus;
  error?: string;
  right: {
    sphere: string;
    cylinder: string;
    axis: string;
  };
  left: {
    sphere: string;
    cylinder: string;
    axis: string;
  };
};

export type LogStatus = "success" | "warning" | "error";

export type LogEntry = {
  id: string;
  time: string;
  status: LogStatus;
  message: string;
  detail: string;
};
