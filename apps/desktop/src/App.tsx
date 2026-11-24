import React, { useCallback, useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  Download,
  Eye,
  EyeOff,
  FlaskConical,
  Folder,
  FolderOpen,
  FolderSearch,
  GitBranch,
  Pause,
  Play,
  Plus,
  RotateCcw,
  Search,
  Trash2,
} from "lucide-react";
import { ateneaApi, isTauriRuntime } from "./api";
import {
  AddFolderModal,
  CreateVariationModal,
  RemoveFolderModal,
} from "./components/FolderModals";
import {
  displayBranchLabel,
  GitKrakenGraph,
} from "./components/GitKrakenGraph";
import { TitleBar } from "./components/TitleBar";
import { UpdateModal } from "./components/UpdateModal";
import { VisualDiffStage } from "./components/VisualDiffStage";
import type {
  AvailableUpdate,
  CommitItem,
  DashboardState,
  ProjectCard,
  ProjectGraph,
  VisualDiffReport,
  WatchedFolder,
} from "./types";

function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const kb = 1024;
  const mb = kb * 1024;
  const gb = mb * 1024;
  if (bytes >= gb) return `${(bytes / gb).toFixed(2)} GB`;
  if (bytes >= mb) return `${(bytes / mb).toFixed(2)} MB`;
  if (bytes >= kb) return `${(bytes / kb).toFixed(1)} KB`;
  return `${bytes} B`;
}

function folderBasename(p: string): string {
  const parts = p.replace(/\\/g, "/").replace(/\/+$/, "").split("/");
  return parts[parts.length - 1] || p;
}

function isInsideFolder(filePath: string, folderPath: string): boolean {
  const f = filePath.replace(/\//g, "\\").toLowerCase();
  const root = folderPath.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
  return f === root || f.startsWith(`${root}\\`);
}

export const App: React.FC = () => {
  const [dashboard, setDashboard] = useState<DashboardState | null>(null);
  const [selectedFolderFilter, setSelectedFolderFilter] =
    useState<string>("__ALL__");
  const [projectSearch, setProjectSearch] = useState<string>("");
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(null);

  const [graph, setGraph] = useState<ProjectGraph | null>(null);
  const [selectedCommitId, setSelectedCommitId] = useState<string | null>(null);
  const [baseCommitId, setBaseCommitId] = useState<string | null>(null);
  const [diffReport, setDiffReport] = useState<VisualDiffReport | null>(null);
  const [loadingDiff, setLoadingDiff] = useState(false);

  const [addFolderOpen, setAddFolderOpen] = useState(false);
  const [folderToRemove, setFolderToRemove] = useState<WatchedFolder | null>(
    null
  );
  const [variationFromCommit, setVariationFromCommit] =
    useState<CommitItem | null>(null);
  const [updateModalOpen, setUpdateModalOpen] = useState(false);
  const [availableUpdate, setAvailableUpdate] =
    useState<AvailableUpdate | null>(null);

  const [toast, setToast] = useState<{ kind: "ok" | "err"; text: string } | null>(
    null
  );
  const [busy, setBusy] = useState(false);

  const notify = useCallback((kind: "ok" | "err", text: string) => {
    setToast({ kind, text });
    window.setTimeout(() => {
      setToast((prev) => (prev?.text === text ? null : prev));
    }, 4200);
  }, []);

  const loadDashboard = useCallback(
    async (preferredProjectId?: string) => {
      if (!isTauriRuntime()) return;
      try {
        const data = await ateneaApi.getDashboard();
        setDashboard(data);

        const targetId =
          preferredProjectId ??
          selectedProjectId ??
          (data.projects.length > 0 ? data.projects[0].id : null);
        if (targetId && data.projects.some((p) => p.id === targetId)) {
          setSelectedProjectId(targetId);
        } else if (data.projects.length > 0) {
          setSelectedProjectId(data.projects[0].id);
        } else {
          setSelectedProjectId(null);
        }
      } catch (err) {
        notify("err", String(err));
      }
    },
    [notify, selectedProjectId]
  );

  const loadProjectGraph = useCallback(
    async (projectId: string) => {
      if (!isTauriRuntime()) return;
      try {
        const g = await ateneaApi.getProjectGraph(projectId);
        setGraph(g);
        if (g.commits.length > 0) {
          setSelectedCommitId((prev) =>
            prev && g.commits.some((c) => c.id === prev) ? prev : g.commits[0].id
          );
          setBaseCommitId((prev) => {
            if (prev && g.commits.some((c) => c.id === prev)) return prev;
            const firstCommit = g.commits[0];
            if (firstCommit.parent_id) return firstCommit.parent_id;
            return g.commits.length > 1 ? g.commits[1].id : firstCommit.id;
          });
        } else {
          setSelectedCommitId(null);
          setBaseCommitId(null);
        }
      } catch (err) {
        notify("err", String(err));
      }
    },
    [notify]
  );

  useEffect(() => {
    loadDashboard();
  }, []);

  useEffect(() => {
    if (selectedProjectId) {
      loadProjectGraph(selectedProjectId);
    } else {
      setGraph(null);
    }
  }, [selectedProjectId, loadProjectGraph]);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let unlisten: (() => void) | undefined;
    ateneaApi
      .onWatcherEvent((payload) => {
        if (payload.type === "snapshot_created") {
          loadDashboard();
          if (payload.commit.project_id === selectedProjectId) {
            loadProjectGraph(payload.commit.project_id);
          }
          notify("ok", "Snapshot guardado automáticamente.");
        } else if (payload.type === "corrupted_save_detected") {
          loadDashboard();
          notify(
            "err",
            `Guardado corrupto bloqueado (${payload.reason}). Historial intacto.`
          );
        } else if (payload.type === "folder_scanned") {
          window.setTimeout(() => loadDashboard(), 650);
        }
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => {
      if (unlisten) unlisten();
    };
  }, [loadDashboard, loadProjectGraph, notify, selectedProjectId]);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let unlistenAvailable: (() => void) | undefined;
    let unlistenOpenModal: (() => void) | undefined;

    ateneaApi
      .onUpdateAvailable((upd) => {
        setAvailableUpdate(upd);
        notify("ok", `Nueva actualización v${upd.version} disponible.`);
      })
      .then((fn) => {
        unlistenAvailable = fn;
      });

    ateneaApi
      .onOpenUpdaterModal(() => {
        setUpdateModalOpen(true);
      })
      .then((fn) => {
        unlistenOpenModal = fn;
      });

    // Revisión periódica no intrusiva cada 4 horas
    const periodicTimer = window.setInterval(() => {
      ateneaApi
        .checkAppUpdate()
        .then((res) => {
          if (res.update) setAvailableUpdate(res.update);
        })
        .catch(() => {});
    }, 4 * 60 * 60 * 1000);

    return () => {
      if (unlistenAvailable) unlistenAvailable();
      if (unlistenOpenModal) unlistenOpenModal();
      window.clearInterval(periodicTimer);
    };
  }, [notify]);

  const filteredProjects: ProjectCard[] = useMemo(() => {
    if (!dashboard) return [];
    const q = projectSearch.trim().toLowerCase();
    return dashboard.projects.filter((p) => {
      if (
        selectedFolderFilter !== "__ALL__" &&
        !isInsideFolder(p.file_path, selectedFolderFilter)
      ) {
        return false;
      }
      if (!q) return true;
      return (
        p.display_name.toLowerCase().includes(q) ||
        p.file_path.toLowerCase().includes(q)
      );
    });
  }, [dashboard, projectSearch, selectedFolderFilter]);

  useEffect(() => {
    if (
      selectedFolderFilter !== "__ALL__" &&
      filteredProjects.length > 0 &&
      !filteredProjects.some((p) => p.id === selectedProjectId)
    ) {
      setSelectedProjectId(filteredProjects[0].id);
    }
  }, [filteredProjects, selectedFolderFilter, selectedProjectId]);

  const selectedProject: ProjectCard | null = useMemo(
    () =>
      dashboard?.projects.find((p) => p.id === selectedProjectId) ?? null,
    [dashboard, selectedProjectId]
  );

  const commits = graph?.commits ?? [];

  const selectedCommit: CommitItem | null = useMemo(
    () => commits.find((c) => c.id === selectedCommitId) ?? commits[0] ?? null,
    [commits, selectedCommitId]
  );

  const baseCommit: CommitItem | null = useMemo(
    () => commits.find((c) => c.id === baseCommitId) ?? null,
    [commits, baseCommitId]
  );

  useEffect(() => {
    if (
      !isTauriRuntime() ||
      !selectedCommit ||
      !baseCommit ||
      selectedCommit.id === baseCommit.id
    ) {
      setDiffReport(null);
      return;
    }
    let cancelled = false;
    setLoadingDiff(true);
    ateneaApi
      .computeVisualDiff(baseCommit.id, selectedCommit.id)
      .then((rep) => {
        if (!cancelled) setDiffReport(rep);
      })
      .catch((err) => {
        if (!cancelled) notify("err", String(err));
      })
      .finally(() => {
        if (!cancelled) setLoadingDiff(false);
      });
    return () => {
      cancelled = true;
    };
  }, [baseCommit, notify, selectedCommit]);

  const dedupSavingsPct = useMemo(() => {
    if (!dashboard || dashboard.stats.total_logical_bytes === 0) return 0;
    const saved =
      1 -
      dashboard.stats.total_physical_bytes / dashboard.stats.total_logical_bytes;
    return Math.max(0, saved * 100);
  }, [dashboard]);

  const countProjectsInFolder = useCallback(
    (folderPath: string): number => {
      if (!dashboard) return 0;
      return dashboard.projects.filter((p) =>
        isInsideFolder(p.file_path, folderPath)
      ).length;
    },
    [dashboard]
  );

  const handleQuickAddFolder = async () => {
    try {
      const picked = await ateneaApi.pickFolderDialog();
      if (picked) {
        setBusy(true);
        await ateneaApi.addWatchedFolder(picked);
        setSelectedFolderFilter(picked);
        notify("ok", `Vigilando ${folderBasename(picked)}`);
        window.setTimeout(() => loadDashboard(), 700);
      } else {
        setAddFolderOpen(true);
      }
    } catch {
      setAddFolderOpen(true);
    } finally {
      setBusy(false);
    }
  };

  const handleConfirmManualPath = async (path: string) => {
    try {
      setBusy(true);
      const wf = await ateneaApi.addWatchedFolder(path);
      setSelectedFolderFilter(wf.path);
      notify("ok", `Vigilando ${folderBasename(wf.path)}`);
      window.setTimeout(() => loadDashboard(), 700);
    } catch (err) {
      notify("err", String(err));
      throw err;
    } finally {
      setBusy(false);
    }
  };

  const handleToggleFolderEnabled = async (wf: WatchedFolder) => {
    try {
      await ateneaApi.toggleWatchedFolder(wf.id, wf.path, !wf.enabled);
      await loadDashboard();
    } catch (err) {
      notify("err", String(err));
    }
  };

  const handleConfirmRemoveFolder = async (
    wf: WatchedFolder,
    purgeSnapshots: boolean
  ) => {
    try {
      setBusy(true);
      const rep = await ateneaApi.removeWatchedFolder(
        wf.id,
        wf.path,
        purgeSnapshots
      );
      if (selectedFolderFilter === wf.path) {
        setSelectedFolderFilter("__ALL__");
      }
      await loadDashboard();
      if (purgeSnapshots) {
        notify(
          "ok",
          `Carpeta quitada y ${formatBytes(rep.freed_bytes)} liberados.`
        );
      } else {
        notify("ok", `Se dejó de vigilar "${folderBasename(wf.path)}".`);
      }
    } catch (err) {
      notify("err", String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleDeleteProject = async (proj: ProjectCard) => {
    const ok = window.confirm(
      `¿Quitar "${proj.display_name}" del historial de Atenea?\n\nTu archivo original en disco (${proj.file_path}) no se toca.`
    );
    if (!ok) return;
    try {
      setBusy(true);
      const rep = await ateneaApi.deleteProject(proj.id);
      await loadDashboard();
      notify(
        "ok",
        `Historial eliminado (${formatBytes(rep.freed_bytes)} liberados).`
      );
    } catch (err) {
      notify("err", String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleSeedDemo = async () => {
    try {
      setBusy(true);
      const pid = await ateneaApi.seedDemoProject();
      setSelectedFolderFilter("__ALL__");
      await loadDashboard(pid);
      await loadProjectGraph(pid);
      notify("ok", "Obra de demostración lista.");
    } catch (err) {
      notify("err", String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleManualSnapshot = async () => {
    if (!selectedProject) return;
    try {
      setBusy(true);
      await ateneaApi.snapshotNow(selectedProject.file_path);
      await loadDashboard(selectedProject.id);
      await loadProjectGraph(selectedProject.id);
      notify("ok", "Snapshot registrado.");
    } catch (err) {
      notify("err", String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleCreateVariation = async (
    name: string,
    checkoutToDisk: boolean
  ) => {
    if (!selectedProject || !variationFromCommit) return;
    try {
      setBusy(true);
      const created = await ateneaApi.createVariation(
        selectedProject.id,
        name,
        variationFromCommit.id,
        checkoutToDisk
      );
      await loadDashboard(selectedProject.id);
      await loadProjectGraph(selectedProject.id);
      notify("ok", `Variación "${created.name}" activa.`);
    } catch (err) {
      notify("err", String(err));
      throw err;
    } finally {
      setBusy(false);
    }
  };

  const handleSwitchVariation = async (branchName: string) => {
    if (!selectedProject) return;
    const checkout = window.confirm(
      `¿Activar la variación "${displayBranchLabel(branchName)}" y cargar su último estado en el archivo .clip (con Clip Studio cerrado)?`
    );
    if (!checkout) return;
    try {
      setBusy(true);
      await ateneaApi.switchVariation(selectedProject.id, branchName, true);
      await loadDashboard(selectedProject.id);
      await loadProjectGraph(selectedProject.id);
      notify("ok", `Variación "${displayBranchLabel(branchName)}" activa.`);
    } catch (err) {
      notify("err", String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleRestoreCopy = async (commit: CommitItem) => {
    if (!selectedProject) return;
    const dotIdx = selectedProject.file_path.lastIndexOf(".");
    const baseName =
      dotIdx > 0
        ? selectedProject.file_path.slice(0, dotIdx)
        : selectedProject.file_path;
    const suggested = `${baseName}_${commit.branch_name}_${commit.id.slice(
      0,
      6
    )}.clip`;
    try {
      const dest = await ateneaApi.pickRestoreDestination(suggested);
      if (!dest) return;
      setBusy(true);
      const savedPath = await ateneaApi.restoreCopy(commit.id, dest);
      notify("ok", `Copia exportada: ${savedPath}`);
      await ateneaApi.revealInExplorer(savedPath);
    } catch (err) {
      notify("err", String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleRestoreInPlace = async (commit: CommitItem) => {
    if (!selectedProject) return;
    const ok = window.confirm(
      `¿Restaurar este punto directamente sobre "${selectedProject.display_name}.clip"?\n\nAsegúrate de haber cerrado el archivo en Clip Studio Paint.`
    );
    if (!ok) return;
    try {
      setBusy(true);
      await ateneaApi.restoreInPlace(commit.id);
      await loadDashboard(selectedProject.id);
      await loadProjectGraph(selectedProject.id);
      notify("ok", "Archivo principal restaurado y verificado.");
    } catch (err) {
      notify("err", String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleTogglePin = async (commitId: string, pinned: boolean) => {
    try {
      await ateneaApi.toggleCommitPin(commitId, pinned);
      if (selectedProjectId) await loadProjectGraph(selectedProjectId);
    } catch (err) {
      notify("err", String(err));
    }
  };

  const handleSaveLabel = async (commitId: string, label: string | null) => {
    try {
      await ateneaApi.setCommitLabel(commitId, label);
      if (selectedProjectId) await loadProjectGraph(selectedProjectId);
    } catch (err) {
      notify("err", String(err));
    }
  };

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden border border-[#e2e4e8] bg-white text-[#18181b]">
      {/* Barra de ventana integrada blanca/perla (reemplaza la barra negra nativa de Windows) */}
      <TitleBar
        logicalBytes={dashboard?.stats.total_logical_bytes ?? 0}
        physicalBytes={dashboard?.stats.total_physical_bytes ?? 0}
        savingsPct={dedupSavingsPct}
        snapshotCount={dashboard?.stats.snapshot_count ?? 0}
        availableUpdate={availableUpdate}
        onAddFolder={handleQuickAddFolder}
        onSeedDemo={handleSeedDemo}
        onOpenUpdates={() => setUpdateModalOpen(true)}
      />

      {/* Modales */}
      <UpdateModal
        open={updateModalOpen}
        initialUpdate={availableUpdate}
        onClose={() => setUpdateModalOpen(false)}
        onUpdateDetected={setAvailableUpdate}
        onSnapshotsFlushed={() => {
          loadDashboard();
          if (selectedProjectId) loadProjectGraph(selectedProjectId);
        }}
      />
      <AddFolderModal
        open={addFolderOpen}
        onClose={() => setAddFolderOpen(false)}
        onBrowseNative={ateneaApi.pickFolderDialog}
        onConfirmPath={handleConfirmManualPath}
      />

      <RemoveFolderModal
        folder={folderToRemove}
        projectCountInFolder={
          folderToRemove ? countProjectsInFolder(folderToRemove.path) : 0
        }
        onClose={() => setFolderToRemove(null)}
        onConfirmRemove={handleConfirmRemoveFolder}
      />

      <CreateVariationModal
        open={Boolean(variationFromCommit)}
        fromCommit={variationFromCommit}
        onClose={() => setVariationFromCommit(null)}
        onConfirm={handleCreateVariation}
      />

      {/* Toast discreto */}
      {toast && (
        <div className="fixed right-4 bottom-4 z-50 flex items-center gap-2 rounded-xl border border-[#e5e7eb] bg-white px-3.5 py-2 text-xs text-[#18181b] shadow-[0_8px_24px_rgba(0,0,0,0.08)]">
          {toast.kind === "ok" ? (
            <CheckCircle2 className="h-3.5 w-3.5 shrink-0 text-[#16a34a]" />
          ) : (
            <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-[#dc2626]" />
          )}
          <span>{toast.text}</span>
        </div>
      )}

      {/* 3 columnas estilo Antigravity */}
      <div className="flex flex-1 overflow-hidden">
        {/* Columna 1: Sidebar sutil (#f8f9fa) */}
        <aside className="flex w-60 shrink-0 flex-col border-r border-[#eceef1] bg-[#f8f9fa]">
          <div className="p-3 space-y-1.5">
            {/* Réplica exacta del botón "+ New Conversation" de Antigravity */}
            <button
              type="button"
              disabled={busy}
              onClick={handleQuickAddFolder}
              className="flex w-full items-center gap-2 rounded-lg border border-[#e4e6ea] bg-white px-3 py-1.5 text-[12.5px] font-medium text-[#18181b] shadow-[0_1px_2px_rgba(0,0,0,0.02)] transition hover:bg-[#f3f4f6] disabled:opacity-50"
            >
              <Plus className="h-3.5 w-3.5 text-[#5f6368]" />
              <span>Vigilar carpeta</span>
            </button>

            <button
              type="button"
              disabled={busy}
              onClick={handleSeedDemo}
              className="flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-[12px] text-[#5f6368] transition hover:bg-[#eceef1] hover:text-[#18181b] disabled:opacity-50"
            >
              <FlaskConical className="h-3.5 w-3.5 text-[#717680]" />
              <span>Generar obra de prueba</span>
            </button>
          </div>

          {/* Lista de Carpetas sin recuadros pesados */}
          <div className="px-3 pt-1 pb-2">
            <div className="mb-1 flex items-center justify-between px-2 text-[11px] text-[#8a8f98]">
              <span>Carpetas</span>
              <span>{dashboard?.watched_folders.length ?? 0}</span>
            </div>

            <div className="max-h-40 space-y-0.5 overflow-y-auto">
              <button
                type="button"
                onClick={() => setSelectedFolderFilter("__ALL__")}
                className={`flex w-full items-center justify-between rounded-md px-2 py-1.5 text-[12.5px] transition ${
                  selectedFolderFilter === "__ALL__"
                    ? "bg-[#eaecef] font-medium text-[#18181b]"
                    : "text-[#5f6368] hover:bg-[#eff1f4] hover:text-[#18181b]"
                }`}
              >
                <span className="flex items-center gap-2 truncate">
                  <Folder className="h-3.5 w-3.5 shrink-0 text-[#717680]" />
                  <span>Todas</span>
                </span>
                <span className="text-[11px] text-[#9ca3af] tabular-nums">
                  {dashboard?.projects.length ?? 0}
                </span>
              </button>

              {dashboard?.watched_folders.map((wf) => {
                const isSelected = selectedFolderFilter === wf.path;
                return (
                  <div
                    key={wf.id}
                    className={`group flex items-center justify-between gap-1 rounded-md px-2 py-1.5 text-[12.5px] transition ${
                      isSelected
                        ? "bg-[#eaecef] font-medium text-[#18181b]"
                        : "text-[#4b5563] hover:bg-[#eff1f4] hover:text-[#18181b]"
                    }`}
                  >
                    <button
                      type="button"
                      onClick={() => setSelectedFolderFilter(wf.path)}
                      className="flex min-w-0 flex-1 items-center gap-2 text-left"
                      title={wf.path}
                    >
                      <Folder className="h-3.5 w-3.5 shrink-0 text-[#717680]" />
                      <span
                        className={`truncate ${
                          !wf.enabled ? "text-[#9ca3af] italic" : ""
                        }`}
                      >
                        {folderBasename(wf.path)}
                      </span>
                    </button>

                    <div className="flex shrink-0 items-center opacity-0 group-hover:opacity-100 transition">
                      <button
                        type="button"
                        onClick={() => handleToggleFolderEnabled(wf)}
                        className="rounded p-1 text-[#717680] hover:bg-black/6 hover:text-[#18181b]"
                        title={wf.enabled ? "Pausar" : "Reanudar"}
                      >
                        {wf.enabled ? (
                          <Pause className="h-3 w-3" />
                        ) : (
                          <Play className="h-3 w-3" />
                        )}
                      </button>
                      <button
                        type="button"
                        onClick={() => ateneaApi.revealInExplorer(wf.path)}
                        className="rounded p-1 text-[#717680] hover:bg-black/6 hover:text-[#18181b]"
                        title="Abrir en Explorador"
                      >
                        <FolderOpen className="h-3 w-3" />
                      </button>
                      <button
                        type="button"
                        onClick={() => setFolderToRemove(wf)}
                        className="rounded p-1 text-[#717680] hover:bg-black/6 hover:text-[#b91c1c]"
                        title="Quitar carpeta"
                      >
                        <Trash2 className="h-3 w-3" />
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>
          </div>

          {/* Lista de Obras limpia estilo Projects de Antigravity */}
          <div className="flex flex-1 flex-col overflow-hidden border-t border-[#eceef1] px-3 pt-3 pb-3">
            <div className="mb-1.5 flex items-center justify-between px-2 text-[11px] text-[#8a8f98]">
              <span>Obras</span>
              <span>{filteredProjects.length}</span>
            </div>

            {(dashboard?.projects.length ?? 0) > 2 && (
              <div className="relative mb-2 px-1">
                <Search className="pointer-events-none absolute top-1/2 left-3 h-3 w-3 -translate-y-1/2 text-[#9ca3af]" />
                <input
                  type="text"
                  value={projectSearch}
                  onChange={(e) => setProjectSearch(e.target.value)}
                  placeholder="Filtrar…"
                  className="w-full rounded-md border border-transparent bg-[#eef0f3] py-1 pr-2 pl-7 text-xs text-[#18181b] placeholder:text-[#9ca3af] focus:border-[#d8dce3] focus:bg-white focus:outline-none"
                />
              </div>
            )}

            <div className="flex-1 space-y-0.5 overflow-y-auto">
              {filteredProjects.map((proj) => {
                const active = proj.id === selectedProjectId;
                return (
                  <div
                    key={proj.id}
                    onClick={() => setSelectedProjectId(proj.id)}
                    className={`group flex cursor-pointer items-center gap-2.5 rounded-md px-2 py-1.5 transition ${
                      active
                        ? "bg-[#eaecef] text-[#18181b]"
                        : "text-[#4b5563] hover:bg-[#eff1f4] hover:text-[#18181b]"
                    }`}
                  >
                    <div className="h-8 w-8 shrink-0 overflow-hidden rounded border border-[#e2e5e9] bg-white">
                      {proj.latest_preview_data_url && (
                        <img
                          src={proj.latest_preview_data_url}
                          alt=""
                          className="h-full w-full object-cover"
                        />
                      )}
                    </div>

                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-1">
                        <p
                          className={`truncate text-[12.5px] ${
                            active ? "font-medium text-[#18181b]" : ""
                          }`}
                        >
                          {proj.display_name}
                        </p>
                        {proj.last_integrity_error && (
                          <AlertTriangle className="h-3 w-3 shrink-0 text-[#dc2626]" />
                        )}
                      </div>
                      <p className="truncate text-[10.5px] text-[#8a8f98] tabular-nums">
                        {proj.snapshot_count} snaps
                        {proj.branch_count > 1
                          ? ` · ${proj.branch_count} var.`
                          : ` · ${formatBytes(proj.latest_file_size)}`}
                      </p>
                    </div>

                    <button
                      type="button"
                      onClick={(e) => {
                        e.stopPropagation();
                        handleDeleteProject(proj);
                      }}
                      className="rounded p-1 text-[#9ca3af] opacity-0 group-hover:opacity-100 hover:bg-black/6 hover:text-[#b91c1c] transition"
                      title="Quitar del historial"
                    >
                      <Trash2 className="h-3 w-3" />
                    </button>
                  </div>
                );
              })}

              {filteredProjects.length === 0 && (
                <p className="px-2 py-6 text-xs text-[#9ca3af]">
                  Sin archivos .clip todavía.
                </p>
              )}
            </div>
          </div>
        </aside>

        {/* Columnas 2 y 3 */}
        {!selectedProject ? (
          <main className="flex flex-1 flex-col items-center justify-center bg-white p-10 text-center">
            <div className="max-w-sm">
              <h2 className="text-[15px] font-semibold text-[#18181b]">
                Selecciona una carpeta de ilustración
              </h2>
              <p className="mt-1.5 text-xs leading-relaxed text-[#6b7280]">
                Elige dónde guardas tus archivos de Clip Studio Paint o carga una obra de prueba para explorar el grafo de versiones y el comparador visual.
              </p>
              <div className="mt-5 flex items-center justify-center gap-2">
                <button
                  type="button"
                  onClick={handleQuickAddFolder}
                  className="rounded-lg bg-[#18181b] px-3.5 py-1.5 text-xs font-medium text-white transition hover:bg-[#27272a]"
                >
                  Vigilar carpeta
                </button>
                <button
                  type="button"
                  onClick={handleSeedDemo}
                  className="rounded-lg border border-[#e2e5e9] bg-white px-3.5 py-1.5 text-xs font-medium text-[#18181b] transition hover:bg-[#f6f7f9]"
                >
                  Cargar obra de prueba
                </button>
              </div>
            </div>
          </main>
        ) : (
          <div className="flex flex-1 flex-col overflow-hidden bg-white">
            {selectedProject.last_integrity_error && (
              <div className="flex items-center justify-between gap-4 border-b border-[#f3d2d5] bg-[#fffafa] px-4 py-2 text-xs text-[#991b1b]">
                <div className="flex items-center gap-2">
                  <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-[#dc2626]" />
                  <span>
                    Guardado incompleto bloqueado ({selectedProject.last_integrity_error}). Tus versiones anteriores están intactas.
                  </span>
                </div>
                {commits[0] && (
                  <button
                    type="button"
                    onClick={() => handleRestoreCopy(commits[0])}
                    className="shrink-0 rounded-md bg-[#18181b] px-2.5 py-1 font-medium text-white hover:bg-[#27272a]"
                  >
                    Exportar última sana
                  </button>
                )}
              </div>
            )}

            <div className="flex flex-1 overflow-hidden">
              {/* Columna 2: Grafo editorial */}
              <GitKrakenGraph
                projectName={selectedProject.display_name}
                branches={graph?.branches ?? []}
                activeBranch={
                  graph?.active_branch ?? selectedProject.active_branch
                }
                commits={commits}
                selectedCommitId={selectedCommit?.id ?? null}
                baseCommitId={baseCommit?.id ?? null}
                busy={busy}
                onSelectCommit={setSelectedCommitId}
                onSetBaseCommit={setBaseCommitId}
                onTogglePin={handleTogglePin}
                onSaveLabel={handleSaveLabel}
                onManualSnapshot={handleManualSnapshot}
                onOpenNewVariation={(commit) => setVariationFromCommit(commit)}
                onSwitchVariation={handleSwitchVariation}
              />

              {/* Columna 3: Escenario + Inspector limpio */}
              <div className="flex flex-1 overflow-hidden bg-white">
                <main className="flex flex-1 flex-col overflow-hidden">
                  {selectedCommit ? (
                    <VisualDiffStage
                      selectedCommit={selectedCommit}
                      baseCommit={baseCommit}
                      diffReport={diffReport}
                      loadingDiff={loadingDiff}
                      onSwapComparison={() => {
                        if (!baseCommit) return;
                        const prevSel = selectedCommit.id;
                        setSelectedCommitId(baseCommit.id);
                        setBaseCommitId(prevSel);
                      }}
                      onClearComparison={() => setBaseCommitId(selectedCommit.id)}
                    />
                  ) : (
                    <div className="flex flex-1 items-center justify-center bg-[#fafafa] text-xs text-[#8a8f98]">
                      Selecciona un snapshot en el grafo.
                    </div>
                  )}
                </main>

                {/* Inspector Derecho: tipográfico, sin cajas pesadas */}
                {selectedCommit && (
                  <aside className="flex w-68 shrink-0 flex-col overflow-y-auto border-l border-[#eceef1] bg-white">
                    {/* Cabecera alineada a 42px */}
                    <div className="flex h-10.5 shrink-0 items-center justify-between border-b border-[#eceef1] px-4 text-[12px]">
                      <span className="font-medium text-[#18181b]">
                        Inspector
                      </span>
                      <span className="font-mono text-[11px] text-[#9ca3af]">
                        {selectedCommit.id.slice(0, 7)} ·{" "}
                        {formatBytes(selectedCommit.file_size)}
                      </span>
                    </div>

                    {/* Acciones ejecutivas sobrias */}
                    <div className="border-b border-[#f0f1f4] p-4 space-y-2">
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => handleRestoreCopy(selectedCommit)}
                        className="flex w-full items-center justify-center gap-2 rounded-lg bg-[#18181b] px-3 py-1.5 text-xs font-medium text-white transition hover:bg-[#27272a] disabled:opacity-50"
                      >
                        <Download className="h-3.5 w-3.5" />
                        Exportar copia (.clip)
                      </button>

                      <div className="grid grid-cols-3 gap-1.5">
                        <button
                          type="button"
                          disabled={busy}
                          onClick={() => setVariationFromCommit(selectedCommit)}
                          className="flex items-center justify-center gap-1 rounded-lg border border-[#e4e6ea] bg-white px-2 py-1.5 text-[11px] text-[#374151] transition hover:bg-[#f6f7f9]"
                          title="Crear variación desde este snapshot"
                        >
                          <GitBranch className="h-3 w-3 text-[#6b7280]" />
                          Rama
                        </button>

                        <button
                          type="button"
                          disabled={busy}
                          onClick={() => handleRestoreInPlace(selectedCommit)}
                          className="flex items-center justify-center gap-1 rounded-lg border border-[#e4e6ea] bg-white px-2 py-1.5 text-[11px] text-[#374151] transition hover:bg-[#f6f7f9]"
                          title="Restaurar en el archivo principal"
                        >
                          <RotateCcw className="h-3 w-3 text-[#6b7280]" />
                          Cargar
                        </button>

                        <button
                          type="button"
                          onClick={() =>
                            ateneaApi.revealInExplorer(
                              selectedProject.file_path
                            )
                          }
                          className="flex items-center justify-center gap-1 rounded-lg border border-[#e4e6ea] bg-white px-2 py-1.5 text-[11px] text-[#374151] transition hover:bg-[#f6f7f9]"
                          title="Mostrar archivo en el Explorador"
                        >
                          <FolderSearch className="h-3 w-3 text-[#6b7280]" />
                          Archivo
                        </button>
                      </div>
                    </div>

                    {/* Cambios de capas sin cajas chillonas */}
                    {diffReport && diffReport.layer_changes.length > 0 && (
                      <div className="border-b border-[#f0f1f4] px-4 py-3.5">
                        <div className="mb-2 text-[11px] text-[#8a8f98]">
                          Cambios de capas (A → B)
                        </div>
                        <ul className="space-y-2">
                          {diffReport.layer_changes.map((lc, i) => (
                            <li
                              key={`${lc.layer_name}-${i}`}
                              className="text-xs leading-snug"
                            >
                              <div className="flex items-center gap-1.5">
                                <span
                                  className={`font-mono text-[11px] font-semibold ${
                                    lc.kind === "added"
                                      ? "text-[#16a34a]"
                                      : lc.kind === "removed"
                                      ? "text-[#dc2626]"
                                      : "text-[#d97706]"
                                  }`}
                                >
                                  {lc.kind === "added"
                                    ? "+"
                                    : lc.kind === "removed"
                                    ? "−"
                                    : "·"}
                                </span>
                                <span className="truncate font-medium text-[#18181b]">
                                  {lc.layer_name}
                                </span>
                              </div>
                              <p className="pl-3 text-[11px] text-[#8a8f98]">
                                {lc.detail}
                              </p>
                            </li>
                          ))}
                        </ul>
                      </div>
                    )}

                    {/* Lista limpia de capas (sin rectángulos grises repetitivos) */}
                    <div className="flex-1 px-4 py-3.5">
                      <div className="mb-2 flex items-center justify-between text-[11px] text-[#8a8f98]">
                        <span>
                          Capas ({selectedCommit.metadata.layers.length})
                        </span>
                        {selectedCommit.metadata.canvas_width && (
                          <span className="tabular-nums">
                            {selectedCommit.metadata.canvas_width}×
                            {selectedCommit.metadata.canvas_height}
                          </span>
                        )}
                      </div>

                      <ul className="divide-y divide-[#f4f5f7]">
                        {[...selectedCommit.metadata.layers]
                          .reverse()
                          .map((layer) => (
                            <li
                              key={layer.id}
                              className="flex items-center justify-between gap-2 py-1.5 text-[12px]"
                            >
                              <div className="flex min-w-0 items-center gap-2">
                                {layer.visible ? (
                                  <Eye className="h-3.5 w-3.5 shrink-0 text-[#717680]" />
                                ) : (
                                  <EyeOff className="h-3.5 w-3.5 shrink-0 text-[#c4c7cc]" />
                                )}
                                <span
                                  className={`truncate ${
                                    layer.visible
                                      ? "text-[#18181b]"
                                      : "text-[#9ca3af] line-through"
                                  }`}
                                >
                                  {layer.name}
                                </span>
                              </div>
                              <span className="shrink-0 font-mono text-[11px] text-[#8a8f98] tabular-nums">
                                {layer.opacity}%
                              </span>
                            </li>
                          ))}
                      </ul>
                    </div>
                  </aside>
                )}
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
