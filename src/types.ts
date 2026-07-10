/** Mục điều hướng chính trên sidebar. */
export type SidebarNavKey = "kr-800" | "settings";

/** Máy đo trên sidebar (có thể mở rộng thêm model khác sau này). */
export type DeviceMenuKey = Extract<SidebarNavKey, "kr-800">;

export type DeviceMenuItem = {
  key: DeviceMenuKey;
  label: string;
  description: string;
};

export type SidebarNavItem = {
  key: SidebarNavKey;
  label: string;
  description: string;
  /** Nhóm hiển thị trên sidebar. */
  section: "device" | "system";
};

/** Tab chức năng trong workspace của một máy (không gồm cấu hình toàn app). */
export type WorkspaceTabKey =
  | "dashboard"
  | "xml-folder"
  | "sync"
  | "logs"
  | "license";

export type WorkspaceTab = {
  key: WorkspaceTabKey;
  label: string;
  description: string;
};

/** @deprecated Dùng SidebarNavKey / WorkspaceTabKey */
export type MenuItemKey = WorkspaceTabKey | "his-settings";
export type HomeMenuItem = WorkspaceTab;

export type SyncStat = {
  label: string;
  value: string;
  tone: "neutral" | "good" | "warning" | "danger";
  description?: string;
};

export type ConnectionState = "connected" | "warning" | "error" | "idle";

/** Connection config from SQLite `app_config` (singleton). */
export type AppSettings = {
  hisApiUrl: string;
  dsCoSoKcbId: number;
  copyRefractionToNewGlasses: boolean;
  username: string;
  /** Empty on save keeps the previously stored password. */
  password: string;
  updatedAt?: string | null;
};

/** XML folder prefs — legacy local UI (KR-800 dùng device tracking SQLite). */
export type XmlFolderState = {
  xmlFolder: string | null;
  autoSyncEnabled: boolean;
};

/** Trạng thái file XML trong SQLite `xml_files`. */
export type TrackedXmlStatus =
  | "waiting"
  | "processing"
  | "parsed"
  | "patient_matched"
  | "mapped"
  | "sending"
  | "processed"
  | "patient_not_found"
  | "treatment_ambiguous"
  | "xml_error"
  | "mapping_error"
  | "send_error"
  | "failed";

export type TrackedXmlFile = {
  id: number;
  deviceKey: string;
  fileName: string;
  filePath: string;
  fileSize?: number | null;
  fileModifiedAt?: string | null;
  status: TrackedXmlStatus;
  errorMessage?: string | null;
  /**
   * Thời gian tạo file parse từ tên (vd. `..._20260707_145000_...` → `2026-07-07 14:50:00`).
   * Dùng để lọc khoảng thời gian xử lý.
   */
  createdAt: string;
  updatedAt: string;
};

export type DeviceFolderState = {
  deviceKey: string;
  trackingFolder?: string | null;
  updatedAt?: string | null;
};

export type FolderScanResult = {
  trackingFolder: string;
  scannedCount: number;
  insertedCount: number;
  files: TrackedXmlFile[];
};

export type SyncSummary = {
  scannedFiles: number;
  sentResults: number;
  skippedFiles: number;
  failedFiles: number;
};

export type SyncFileStatus = "waiting" | "sent" | "needs-review" | "failed";

export type EyeRefraction = {
  sphere: string;
  cylinder: string;
  axis: string;
};

export type SyncFileRow = {
  id: string;
  fileName: string;
  patientId: string;
  measuredAt: string;
  status: SyncFileStatus;
  error?: string;
  right: EyeRefraction;
  left: EyeRefraction;
};

export type XmlPreview = {
  fileName: string;
  patientId?: string | null;
  measuredAt?: string | null;
  right: {
    sphere?: string | null;
    cylinder?: string | null;
    axis?: string | null;
  };
  left: {
    sphere?: string | null;
    cylinder?: string | null;
    axis?: string | null;
  };
};

export type LogStatus = "success" | "warning" | "error";

export type LogEntry = {
  id: string;
  /** ISO date YYYY-MM-DD for filtering */
  date: string;
  time: string;
  status: LogStatus;
  message: string;
  detail: string;
};

export type StatusOverviewItem = {
  label: string;
  value: string;
  tone: "neutral" | "good" | "warning" | "danger";
};

export type AppLogInfo = {
  logDir: string;
  logPath: string;
  sizeBytes: number;
  hasBackup: boolean;
};

export type ExportLogsResult = {
  targetPath: string;
  bytesWritten: number;
  sourceFiles: number;
};

/** Trạng thái đăng nhập HIS (không trả access_token ra UI). */
export type HisAuthStatus = {
  loggedIn: boolean;
  username?: string | null;
  fullName?: string | null;
  coSoKcbId?: number | null;
  tokenType?: string | null;
  expiresIn?: number | null;
  expiration?: string | null;
  updatedAt?: string | null;
  hasAccessToken: boolean;
};
