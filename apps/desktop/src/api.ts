import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import type {
  AvailableUpdate,
  BranchInfo,
  CommitItem,
  DashboardState,
  ProjectGraph,
  RemoveFolderReport,
  SaveUpdaterSettingsInput,
  UpdateCheckResponse,
  UpdatePreflightReport,
  UpdateProgressPayload,
  UpdaterSettings,
  VisualDiffReport,
  WatchedFolder,
  WatcherEventPayload,
} from "./types";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export const ateneaApi = {
  getDashboard(): Promise<DashboardState> {
    return invoke<DashboardState>("get_dashboard_state");
  },

  async pickFolderDialog(): Promise<string | null> {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Selecciona una carpeta con proyectos de Clip Studio Paint (.clip)",
    });
    return typeof selected === "string" ? selected : null;
  },

  async pickRestoreDestination(defaultFilePath: string): Promise<string | null> {
    const chosen = await save({
      defaultPath: defaultFilePath,
      filters: [{ name: "Clip Studio Paint", extensions: ["clip"] }],
      title: "Guardar versión restaurada como copia",
    });
    return typeof chosen === "string" ? chosen : null;
  },

  addWatchedFolder(path: string): Promise<WatchedFolder> {
    return invoke<WatchedFolder>("add_watched_folder", { path });
  },

  toggleWatchedFolder(
    folderId: number,
    path: string,
    enabled: boolean
  ): Promise<WatchedFolder> {
    return invoke<WatchedFolder>("toggle_watched_folder", {
      folderId,
      path,
      enabled,
    });
  },

  removeWatchedFolder(
    folderId: number,
    path: string,
    purgeSnapshots: boolean
  ): Promise<RemoveFolderReport> {
    return invoke<RemoveFolderReport>("remove_watched_folder", {
      folderId,
      path,
      purgeSnapshots,
    });
  },

  deleteProject(projectId: string): Promise<RemoveFolderReport> {
    return invoke<RemoveFolderReport>("delete_project", { projectId });
  },

  getProjectGraph(projectId: string): Promise<ProjectGraph> {
    return invoke<ProjectGraph>("get_project_graph", { projectId });
  },

  createVariation(
    projectId: string,
    name: string,
    fromCommitId: string,
    checkoutToDisk: boolean
  ): Promise<BranchInfo> {
    return invoke<BranchInfo>("create_variation", {
      projectId,
      name,
      fromCommitId,
      checkoutToDisk,
    });
  },

  switchVariation(
    projectId: string,
    branchName: string,
    checkoutToDisk: boolean
  ): Promise<void> {
    return invoke<void>("switch_variation", {
      projectId,
      branchName,
      checkoutToDisk,
    });
  },

  snapshotNow(filePath: string, label?: string): Promise<unknown> {
    return invoke("snapshot_project_now", {
      filePath,
      label: label && label.trim() ? label.trim() : null,
    });
  },

  setCommitLabel(commitId: string, label: string | null): Promise<CommitItem> {
    return invoke<CommitItem>("set_commit_label", { commitId, label });
  },

  toggleCommitPin(commitId: string, pinned: boolean): Promise<CommitItem> {
    return invoke<CommitItem>("toggle_commit_pin", { commitId, pinned });
  },

  computeVisualDiff(
    baseCommitId: string,
    targetCommitId: string
  ): Promise<VisualDiffReport> {
    return invoke<VisualDiffReport>("compute_visual_diff", {
      baseCommitId,
      targetCommitId,
    });
  },

  restoreCopy(commitId: string, destinationPath: string): Promise<string> {
    return invoke<string>("restore_commit_copy", { commitId, destinationPath });
  },

  restoreInPlace(commitId: string): Promise<string> {
    return invoke<string>("restore_commit_in_place", { commitId });
  },

  revealInExplorer(path: string): Promise<void> {
    return invoke<void>("reveal_in_explorer", { path });
  },

  seedDemoProject(): Promise<string> {
    return invoke<string>("seed_demo_project");
  },

  onWatcherEvent(
    handler: (payload: WatcherEventPayload) => void
  ): Promise<UnlistenFn> {
    return listen<WatcherEventPayload>("atenea://watcher-event", (ev) => {
      handler(ev.payload);
    });
  },

  getUpdaterSettings(): Promise<UpdaterSettings> {
    return invoke<UpdaterSettings>("get_updater_settings");
  },

  saveUpdaterSettings(input: SaveUpdaterSettingsInput): Promise<UpdaterSettings> {
    return invoke<UpdaterSettings>("save_updater_settings", { input });
  },

  checkAppUpdate(channelOverride?: string): Promise<UpdateCheckResponse> {
    return invoke<UpdateCheckResponse>("check_app_update", {
      channelOverride: channelOverride ?? null,
    });
  },

  preflightUpdateCheck(): Promise<UpdatePreflightReport> {
    return invoke<UpdatePreflightReport>("preflight_update_check");
  },

  flushSnapshotsBeforeUpdate(): Promise<UpdatePreflightReport> {
    return invoke<UpdatePreflightReport>("flush_snapshots_before_update");
  },

  installAppUpdate(params: {
    userConfirmedSaved: boolean;
    expectedVersion?: string;
    channelOverride?: string;
  }): Promise<void> {
    return invoke<void>("install_app_update", {
      userConfirmedSaved: params.userConfirmedSaved,
      expectedVersion: params.expectedVersion ?? null,
      channelOverride: params.channelOverride ?? null,
    });
  },

  onUpdateAvailable(
    handler: (payload: AvailableUpdate) => void
  ): Promise<UnlistenFn> {
    return listen<AvailableUpdate>("atenea://update-available", (ev) => {
      handler(ev.payload);
    });
  },

  onUpdateProgress(
    handler: (payload: UpdateProgressPayload) => void
  ): Promise<UnlistenFn> {
    return listen<UpdateProgressPayload>("atenea://update-progress", (ev) => {
      handler(ev.payload);
    });
  },

  onOpenUpdaterModal(handler: () => void): Promise<UnlistenFn> {
    return listen("atenea://open-updater-modal", () => {
      handler();
    });
  },
};
