export interface WatchedFolder {
  id: number;
  path: string;
  added_at: number;
  enabled: boolean;
}

export interface BranchInfo {
  name: string;
  head_commit_id: string | null;
  commit_count: number;
  is_active: boolean;
}

export interface LayerSummary {
  id: string;
  name: string;
  visible: boolean;
  opacity: number;
  is_folder: boolean;
}

export interface AssetMetadata {
  format: string;
  canvas_width: number | null;
  canvas_height: number | null;
  dpi: number | null;
  layers: LayerSummary[];
}

export interface ProjectCard {
  id: string;
  file_path: string;
  display_name: string;
  format: string;
  active_branch: string;
  created_at: number;
  updated_at: number;
  head_commit_id: string | null;
  latest_preview_path: string | null;
  latest_preview_data_url: string | null;
  snapshot_count: number;
  branch_count: number;
  latest_file_size: number;
  total_logical_bytes: number;
  total_stored_bytes: number;
  last_integrity_error: string | null;
}

export interface CommitItem {
  id: string;
  project_id: string;
  parent_id: string | null;
  branch_name: string;
  file_hash: string;
  file_size: number;
  added_bytes: number;
  chunk_count: number;
  preview_hash: string | null;
  preview_path: string | null;
  preview_data_url: string | null;
  metadata: AssetMetadata;
  label: string | null;
  pinned: boolean;
  author_kind: string;
  created_at: number;
}

export interface ProjectGraph {
  project_id: string;
  active_branch: string;
  branches: BranchInfo[];
  commits: CommitItem[];
}

export interface RemoveFolderReport {
  removed_projects: number;
  freed_blobs: number;
  freed_bytes: number;
}

export interface StorageStats {
  project_count: number;
  snapshot_count: number;
  unique_blob_count: number;
  total_logical_bytes: number;
  total_physical_bytes: number;
}

export interface DashboardState {
  store_root: string;
  watched_folders: WatchedFolder[];
  projects: ProjectCard[];
  stats: StorageStats;
}

export interface BoundingBox {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface LayerChange {
  kind: "added" | "removed" | "modified";
  layer_name: string;
  detail: string;
}

export interface VisualDiffReport {
  base_commit_id: string;
  target_commit_id: string;
  preview_width: number;
  preview_height: number;
  changed_pixels: number;
  total_pixels: number;
  changed_ratio: number;
  overall_bbox: BoundingBox | null;
  base_image_data_url: string | null;
  target_image_data_url: string | null;
  heatmap_data_url: string | null;
  layer_changes: LayerChange[];
}

export type WatcherEventPayload =
  | { type: "folder_scanned"; folder: string; discovered_files: number }
  | { type: "snapshot_created"; commit: CommitItem }
  | {
      type: "corrupted_save_detected";
      project_id: string;
      file_path: string;
      reason: string;
    }
  | { type: "watcher_warning"; message: string };

export interface DirtyProjectStatus {
  project_id: string;
  display_name: string;
  file_path: string;
  is_locked_by_app: boolean;
  has_unsnapshotted_disk_changes: boolean;
}

export interface PreUpdateWorkspaceAudit {
  store_root: string;
  project_count: number;
  snapshot_count: number;
  total_physical_bytes: number;
  dirty_or_locked_projects: DirtyProjectStatus[];
}

export interface UpdaterSettings {
  current_version: string;
  channel: "stable" | "canary" | string;
  github_repo: string;
  r2_public_url: string | null;
  custom_pubkey: string | null;
  auto_check: boolean;
  active_adapter_mode: "github" | "r2_with_github_fallback" | "r2_only" | "unconfigured" | string;
  resolved_endpoints: string[];
  using_dev_placeholder_key: boolean;
}

export interface SaveUpdaterSettingsInput {
  channel: string;
  github_repo: string;
  r2_public_url: string | null;
  custom_pubkey: string | null;
  auto_check: boolean;
}

export interface AvailableUpdate {
  version: string;
  current_version: string;
  channel: string;
  pub_date: string | null;
  notes: string | null;
  download_url: string;
}

export interface UpdateCheckResponse {
  current_version: string;
  channel: string;
  active_adapter_mode: string;
  endpoints: string[];
  status: "available" | "up_to_date" | "no_published_release" | "error";
  update: AvailableUpdate | null;
  message: string | null;
  using_dev_placeholder_key: boolean;
}

export interface UpdatePreflightReport {
  clip_studio_running: boolean;
  pending_watcher_files: string[];
  audit: PreUpdateWorkspaceAudit;
  update_in_progress: boolean;
}

export interface UpdateProgressPayload {
  phase:
    | "downloading"
    | "capturing_snapshots"
    | "stopping_watcher"
    | "backing_up_db"
    | "installing"
    | "error";
  downloaded_bytes: number;
  total_bytes: number | null;
  message: string;
}
