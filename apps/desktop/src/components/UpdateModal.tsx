import React, { useCallback, useEffect, useState } from "react";
import {
  AlertTriangle,
  ArrowUpCircle,
  CheckCircle2,
  Cloud,
  Download,
  RefreshCw,
  Save,
  Settings2,
  ShieldCheck,
  X,
} from "lucide-react";
import { ateneaApi, isTauriRuntime } from "../api";
import type {
  AvailableUpdate,
  UpdateCheckResponse,
  UpdatePreflightReport,
  UpdateProgressPayload,
  UpdaterSettings,
} from "../types";

interface UpdateModalProps {
  open: boolean;
  initialUpdate: AvailableUpdate | null;
  onClose: () => void;
  onUpdateDetected: (update: AvailableUpdate | null) => void;
  onSnapshotsFlushed: () => void;
}

function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const kb = 1024;
  const mb = kb * 1024;
  const gb = mb * 1024;
  if (bytes >= gb) return `${(bytes / gb).toFixed(2)} GB`;
  if (bytes >= mb) return `${(bytes / mb).toFixed(1)} MB`;
  if (bytes >= kb) return `${(bytes / kb).toFixed(0)} KB`;
  return `${bytes} B`;
}

function adapterBadgeLabel(mode: string): string {
  switch (mode) {
    case "r2_with_github_fallback":
      return "Cloudflare R2 + Fallback GitHub";
    case "r2_only":
      return "Cloudflare R2";
    case "github":
      return "GitHub Releases";
    default:
      return "Sin configurar";
  }
}

export const UpdateModal: React.FC<UpdateModalProps> = ({
  open,
  initialUpdate,
  onClose,
  onUpdateDetected,
  onSnapshotsFlushed,
}) => {
  const [settings, setSettings] = useState<UpdaterSettings | null>(null);
  const [checkResult, setCheckResult] = useState<UpdateCheckResponse | null>(null);
  const [preflight, setPreflight] = useState<UpdatePreflightReport | null>(null);

  const [checking, setChecking] = useState(false);
  const [flushing, setFlushing] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<UpdateProgressPayload | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Confirmación explícita obligatoria de que el artista guardó su trabajo en Clip Studio Paint
  const [confirmedSavedWork, setConfirmedSavedWork] = useState(false);

  // Panel desplegable para probar canales o configurar el adapter R2
  const [configOpen, setConfigOpen] = useState(false);
  const [channelInput, setChannelInput] = useState<string>("stable");
  const [repoInput, setRepoInput] = useState<string>("");
  const [r2UrlInput, setR2UrlInput] = useState<string>("");
  const [autoCheckInput, setAutoCheckInput] = useState<boolean>(true);
  const [savingSettings, setSavingSettings] = useState(false);

  const refreshPreflight = useCallback(async () => {
    if (!isTauriRuntime()) return;
    try {
      const rep = await ateneaApi.preflightUpdateCheck();
      setPreflight(rep);
    } catch (err) {
      setErrorMsg(String(err));
    }
  }, []);

  const runCheck = useCallback(
    async (channelOverride?: string) => {
      if (!isTauriRuntime()) return;
      setChecking(true);
      setErrorMsg(null);
      try {
        const res = await ateneaApi.checkAppUpdate(channelOverride);
        setCheckResult(res);
        onUpdateDetected(res.update);
      } catch (err) {
        setErrorMsg(String(err));
      } finally {
        setChecking(false);
      }
    },
    [onUpdateDetected]
  );

  useEffect(() => {
    if (!open || !isTauriRuntime()) return;
    setConfirmedSavedWork(false);
    setErrorMsg(null);
    setProgress(null);

    ateneaApi
      .getUpdaterSettings()
      .then((cfg) => {
        setSettings(cfg);
        setChannelInput(cfg.channel);
        setRepoInput(cfg.github_repo);
        setR2UrlInput(cfg.r2_public_url ?? "");
        setAutoCheckInput(cfg.auto_check);
      })
      .catch((err) => setErrorMsg(String(err)));

    refreshPreflight();
    runCheck();
  }, [open, refreshPreflight, runCheck]);

  useEffect(() => {
    if (!open || !isTauriRuntime()) return;
    let unlisten: (() => void) | undefined;
    ateneaApi
      .onUpdateProgress((payload) => {
        setProgress(payload);
        if (payload.phase === "error") {
          setErrorMsg(payload.message);
          setInstalling(false);
        }
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => {
      if (unlisten) unlisten();
    };
  }, [open]);

  if (!open) return null;

  const activeUpdate = checkResult?.update ?? initialUpdate;
  const dirtyProjects = preflight?.audit.dirty_or_locked_projects ?? [];
  const lockedProjects = dirtyProjects.filter((p) => p.is_locked_by_app);
  const unsnapshottedProjects = dirtyProjects.filter(
    (p) => p.has_unsnapshotted_disk_changes
  );
  const pendingWatcherCount = preflight?.pending_watcher_files.length ?? 0;
  const hasActiveCspWarning =
    Boolean(preflight?.clip_studio_running) || lockedProjects.length > 0;
  const hasPendingDiskSnapshots =
    unsnapshottedProjects.length > 0 || pendingWatcherCount > 0;

  const handleFlushPendingSnapshots = async () => {
    setFlushing(true);
    setErrorMsg(null);
    try {
      const updatedReport = await ateneaApi.flushSnapshotsBeforeUpdate();
      setPreflight(updatedReport);
      onSnapshotsFlushed();
    } catch (err) {
      setErrorMsg(String(err));
    } finally {
      setFlushing(false);
    }
  };

  const handleSaveSettings = async (e: React.FormEvent) => {
    e.preventDefault();
    setSavingSettings(true);
    setErrorMsg(null);
    try {
      const saved = await ateneaApi.saveUpdaterSettings({
        channel: channelInput,
        github_repo: repoInput,
        r2_public_url: r2UrlInput.trim() ? r2UrlInput.trim() : null,
        custom_pubkey: settings?.custom_pubkey ?? null,
        auto_check: autoCheckInput,
      });
      setSettings(saved);
      setConfigOpen(false);
      await runCheck(saved.channel);
    } catch (err) {
      setErrorMsg(String(err));
    } finally {
      setSavingSettings(false);
    }
  };

  const handleQuickChannelSwitch = async (nextChannel: "stable" | "canary") => {
    if (!settings) return;
    setChannelInput(nextChannel);
    try {
      const saved = await ateneaApi.saveUpdaterSettings({
        channel: nextChannel,
        github_repo: settings.github_repo,
        r2_public_url: settings.r2_public_url,
        custom_pubkey: settings.custom_pubkey,
        auto_check: settings.auto_check,
      });
      setSettings(saved);
      setConfirmedSavedWork(false);
      await runCheck(nextChannel);
    } catch (err) {
      setErrorMsg(String(err));
    }
  };

  const handleInstallNow = async () => {
    if (!activeUpdate || !confirmedSavedWork) return;
    setInstalling(true);
    setErrorMsg(null);
    try {
      await ateneaApi.installAppUpdate({
        userConfirmedSaved: confirmedSavedWork,
        expectedVersion: activeUpdate.version,
        channelOverride: activeUpdate.channel,
      });
    } catch (err) {
      setErrorMsg(String(err));
      setInstalling(false);
      await refreshPreflight();
    }
  };

  const downloadPct =
    progress && progress.total_bytes && progress.total_bytes > 0
      ? Math.min(100, Math.round((progress.downloaded_bytes / progress.total_bytes) * 100))
      : null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/25 p-4 backdrop-blur-[1.5px]">
      <div className="flex max-h-[90vh] w-full max-w-lg flex-col overflow-hidden rounded-2xl border border-[#e6e8ec] bg-white shadow-2xl">
        {/* Cabecera */}
        <div className="flex items-center justify-between border-b border-[#eceef1] bg-[#f8f9fa] px-5 py-3.5">
          <div className="flex items-center gap-2.5">
            <ArrowUpCircle className="h-4 w-4 text-[#18181b]" />
            <div>
              <h2 className="text-[13.5px] font-semibold text-[#18181b]">
                Actualizaciones de Atenea
              </h2>
              <div className="mt-0.5 flex flex-wrap items-center gap-1.5 text-[11px] text-[#6b7280]">
                <span>Actual: v{settings?.current_version ?? "0.1.0"}</span>
                <span>·</span>
                <span className="inline-flex items-center gap-1 rounded bg-white px-1.5 py-0.5 text-[10.5px] font-medium text-[#374151] border border-[#e2e5e9]">
                  <Cloud className="h-2.5 w-2.5 text-[#5f6368]" />
                  {adapterBadgeLabel(
                    checkResult?.active_adapter_mode ??
                      settings?.active_adapter_mode ??
                      "github"
                  )}
                </span>
              </div>
            </div>
          </div>

          <div className="flex items-center gap-1">
            <button
              type="button"
              disabled={installing}
              onClick={() => setConfigOpen((v) => !v)}
              title="Configurar canal o adapter R2 / GitHub"
              className={`rounded-md p-1.5 text-xs transition ${
                configOpen
                  ? "bg-[#e5e7eb] text-[#18181b]"
                  : "text-[#6b7280] hover:bg-[#eceef1] hover:text-[#18181b]"
              } disabled:opacity-40`}
            >
              <Settings2 className="h-3.5 w-3.5" />
            </button>
            <button
              type="button"
              disabled={installing}
              onClick={onClose}
              className="rounded-md p-1.5 text-[#8a8f98] hover:bg-[#eceef1] hover:text-[#18181b] disabled:opacity-40"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        </div>

        {/* Cuerpo scrollable */}
        <div className="flex-1 space-y-3.5 overflow-y-auto p-5">
          {/* Selector rápido de canal */}
          <div className="flex items-center justify-between rounded-xl border border-[#eceef1] bg-[#f8f9fa] px-3 py-2">
            <span className="text-xs text-[#4b5563]">Canal de lanzamiento</span>
            <div className="flex items-center gap-1 rounded-lg bg-[#eaecef] p-0.5 text-[11.5px]">
              <button
                type="button"
                disabled={installing || checking}
                onClick={() => handleQuickChannelSwitch("stable")}
                className={`rounded-md px-2.5 py-1 transition ${
                  (settings?.channel ?? "stable") === "stable"
                    ? "bg-white font-medium text-[#18181b] shadow-xs"
                    : "text-[#5f6368] hover:text-[#18181b]"
                }`}
              >
                Estable
              </button>
              <button
                type="button"
                disabled={installing || checking}
                onClick={() => handleQuickChannelSwitch("canary")}
                className={`rounded-md px-2.5 py-1 transition ${
                  settings?.channel === "canary"
                    ? "bg-white font-medium text-[#18181b] shadow-xs"
                    : "text-[#5f6368] hover:text-[#18181b]"
                }`}
              >
                Canary (Pruebas)
              </button>
            </div>
          </div>

          {/* Panel opcional de configuración del Adapter (GitHub Releases <-> Cloudflare R2) */}
          {configOpen && (
            <form
              onSubmit={handleSaveSettings}
              className="space-y-3 rounded-xl border border-[#dfe3e8] bg-[#f9fafb] p-3.5"
            >
              <div className="flex items-center justify-between">
                <span className="text-xs font-semibold text-[#18181b]">
                  Origen de releases (Adapter GitHub / Cloudflare R2)
                </span>
              </div>

              <div>
                <label className="block text-[11px] font-medium text-[#4b5563]">
                  Repositorio GitHub (`usuario/repo`)
                </label>
                <input
                  type="text"
                  value={repoInput}
                  onChange={(e) => setRepoInput(e.target.value)}
                  placeholder="fmartinez09/Atenea"
                  className="mt-1 w-full rounded-lg border border-[#e2e5e9] bg-white px-2.5 py-1.5 text-xs text-[#18181b] focus:border-[#9ca3af] focus:outline-none"
                />
              </div>

              <div>
                <label className="block text-[11px] font-medium text-[#4b5563]">
                  URL pública Cloudflare R2 (opcional — déjalo vacío mientras pruebas en GitHub)
                </label>
                <input
                  type="text"
                  value={r2UrlInput}
                  onChange={(e) => setR2UrlInput(e.target.value)}
                  placeholder="https://releases.tudominio.com o https://pub-xxx.r2.dev"
                  className="mt-1 w-full rounded-lg border border-[#e2e5e9] bg-white px-2.5 py-1.5 text-xs text-[#18181b] placeholder:text-[#9ca3af] focus:border-[#9ca3af] focus:outline-none"
                />
                <p className="mt-1 text-[10.5px] text-[#6b7280]">
                  Al completarlo, R2 pasa a ser fuente primaria y GitHub Releases queda como respaldo automático.
                </p>
              </div>

              <label className="flex items-center gap-2 text-xs text-[#4b5563]">
                <input
                  type="checkbox"
                  checked={autoCheckInput}
                  onChange={(e) => setAutoCheckInput(e.target.checked)}
                  className="accent-[#18181b]"
                />
                <span>Buscar nuevas versiones automáticamente al abrir Atenea</span>
              </label>

              {(checkResult?.using_dev_placeholder_key ??
                settings?.using_dev_placeholder_key) && (
                <div className="rounded-lg border border-[#fde68a] bg-[#fffbeb] px-2.5 py-2 text-[11px] text-[#92400e]">
                  Modo desarrollo: aún está activa la llave pública de ejemplo. Ejecuta{" "}
                  <code className="rounded bg-black/5 px-1">npm run updater:keygen</code>{" "}
                  antes de publicar tu primer release firmado.
                </div>
              )}

              <div className="flex justify-end gap-2 pt-1">
                <button
                  type="button"
                  onClick={() => setConfigOpen(false)}
                  className="rounded-lg px-2.5 py-1 text-xs text-[#6b7280] hover:bg-[#eceef1]"
                >
                  Cerrar
                </button>
                <button
                  type="submit"
                  disabled={savingSettings}
                  className="rounded-lg bg-[#18181b] px-3 py-1 text-xs font-medium text-white hover:bg-[#27272a] disabled:opacity-50"
                >
                  {savingSettings ? "Guardando…" : "Guardar origen"}
                </button>
              </div>
            </form>
          )}

          {/* Garantía explícita de preservación del historial local aún no sincronizado */}
          {preflight && (
            <div className="flex items-start gap-2.5 rounded-xl border border-[#bbf7d0] bg-[#f0fdf4] p-3 text-xs text-[#166534]">
              <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0 text-[#16a34a]" />
              <div className="min-w-0 flex-1">
                <p className="font-medium text-[#14532d]">
                  Tu historial local no se borra al actualizar ({preflight.audit.snapshot_count}{" "}
                  snapshots en {preflight.audit.project_count} obras ·{" "}
                  {formatBytes(preflight.audit.total_physical_bytes)})
                </p>
                <p className="mt-0.5 text-[11px] leading-relaxed text-[#15803d]">
                  Aunque aún no esté sincronizado en la nube, tus versiones viven aisladas fuera del instalador y Atenea crea un respaldo verificado del índice antes de aplicar cambios.
                </p>
              </div>
            </div>
          )}

          {/* Estado: Actualización disponible vs Al día */}
          {checking ? (
            <div className="flex items-center justify-center gap-2 rounded-xl border border-[#eceef1] py-8 text-xs text-[#6b7280]">
              <RefreshCw className="h-4 w-4 animate-spin text-[#5f6368]" />
              <span>Consultando canal {settings?.channel ?? "stable"}…</span>
            </div>
          ) : activeUpdate ? (
            <div className="space-y-3">
              {/* Tarjeta del nuevo Release */}
              <div className="rounded-xl border border-[#dfe3e8] bg-white p-3.5">
                <div className="flex items-center justify-between gap-2">
                  <div>
                    <span className="inline-flex items-center rounded-full bg-[#18181b] px-2 py-0.5 text-[10.5px] font-medium text-white">
                      Nueva versión v{activeUpdate.version}
                    </span>
                    {activeUpdate.pub_date && (
                      <span className="ml-2 text-[11px] text-[#8a8f98]">
                        {activeUpdate.pub_date.slice(0, 10)}
                      </span>
                    )}
                  </div>
                  <span className="text-[11px] text-[#6b7280]">
                    v{activeUpdate.current_version} → v{activeUpdate.version}
                  </span>
                </div>

                {activeUpdate.notes && (
                  <div className="mt-2.5 max-h-32 overflow-y-auto rounded-lg border border-[#eceef1] bg-[#f8f9fa] p-2.5 text-[11.5px] whitespace-pre-wrap text-[#374151]">
                    {activeUpdate.notes}
                  </div>
                )}
              </div>

              {/* Auditoría de archivos abiertos / sin guardar en Clip Studio Paint */}
              {!installing && (
                <div className="space-y-2.5 rounded-xl border border-[#fde68a] bg-[#fffbeb] p-3.5 text-xs text-[#92400e]">
                  <div className="flex items-start justify-between gap-2">
                    <div className="flex items-start gap-2">
                      <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-[#d97706]" />
                      <div>
                        <p className="font-semibold text-[#78350f]">
                          Antes de actualizar: ¿ya guardaste tus cambios en Clip Studio Paint?
                        </p>
                        {hasActiveCspWarning ? (
                          <p className="mt-0.5 text-[11px] leading-relaxed">
                            Detectamos{" "}
                            {preflight?.clip_studio_running
                              ? "Clip Studio Paint abierto"
                              : `${lockedProjects.length} archivo(s) .clip en uso`}
                            . Presiona <strong>Ctrl + S</strong> en tu lienzo para que Atenea capture tu último estado antes de reiniciarse.
                          </p>
                        ) : (
                          <p className="mt-0.5 text-[11px] leading-relaxed">
                            Atenea se cerrará unos segundos para actualizarse. Si tienes un dibujo abierto con trazos sin guardar, guárdalo primero con <strong>Ctrl + S</strong>.
                          </p>
                        )}
                      </div>
                    </div>

                    <button
                      type="button"
                      onClick={refreshPreflight}
                      title="Volver a verificar estado de archivos"
                      className="shrink-0 rounded-md border border-[#fcd34d] bg-white px-2 py-1 text-[11px] font-medium text-[#78350f] hover:bg-[#fef3c7]"
                    >
                      Verificar
                    </button>
                  </div>

                  {hasPendingDiskSnapshots && (
                    <div className="flex items-center justify-between gap-2 rounded-lg border border-[#fcd34d] bg-white/80 px-2.5 py-2">
                      <span className="text-[11px] text-[#78350f]">
                        Hay{" "}
                        <strong>
                          {unsnapshottedProjects.length || pendingWatcherCount}
                        </strong>{" "}
                        obra(s) con cambios en disco pendientes de snapshot.
                      </span>
                      <button
                        type="button"
                        disabled={flushing}
                        onClick={handleFlushPendingSnapshots}
                        className="inline-flex shrink-0 items-center gap-1 rounded-md bg-[#18181b] px-2.5 py-1 text-[11px] font-medium text-white hover:bg-[#27272a] disabled:opacity-50"
                      >
                        <Save className="h-3 w-3" />
                        {flushing ? "Guardando…" : "Capturar ahora"}
                      </button>
                    </div>
                  )}

                  {/* Checkbox obligatorio */}
                  <label className="mt-1 flex cursor-pointer items-start gap-2 rounded-lg border border-[#fcd34d] bg-white p-2.5 text-[11.5px] text-[#18181b]">
                    <input
                      type="checkbox"
                      checked={confirmedSavedWork}
                      onChange={(e) => setConfirmedSavedWork(e.target.checked)}
                      className="mt-0.5 accent-[#18181b]"
                    />
                    <span>
                      <strong>Sí, ya guardé mis cambios</strong> (o no tengo lienzos pendientes) y quiero respaldar el índice local y actualizar ahora.
                    </span>
                  </label>
                </div>
              )}

              {/* Progreso en vivo durante la actualización */}
              {installing && (
                <div className="space-y-2 rounded-xl border border-[#e2e5e9] bg-[#f8f9fa] p-3.5">
                  <div className="flex items-center justify-between text-xs font-medium text-[#18181b]">
                    <span className="flex items-center gap-2">
                      <RefreshCw className="h-3.5 w-3.5 animate-spin text-[#18181b]" />
                      {progress?.message ?? "Preparando actualización segura…"}
                    </span>
                    {downloadPct !== null && <span>{downloadPct}%</span>}
                  </div>
                  <div className="h-1.5 w-full overflow-hidden rounded-full bg-[#e5e7eb]">
                    <div
                      className="h-full bg-[#18181b] transition-all duration-150"
                      style={{
                        width: `${
                          downloadPct !== null
                            ? downloadPct
                            : progress?.phase === "installing"
                            ? 100
                            : 65
                        }%`,
                      }}
                    />
                  </div>
                  <p className="text-[11px] text-[#6b7280]">
                    No cierres la ventana: Atenea verificará la firma digital, guardará tus snapshots pendientes y se reiniciará sola.
                  </p>
                </div>
              )}
            </div>
          ) : (
            <div className="rounded-xl border border-[#eceef1] bg-[#f8f9fa] p-4 text-center">
              <CheckCircle2 className="mx-auto h-5 w-5 text-[#16a34a]" />
              <p className="mt-1.5 text-xs font-medium text-[#18181b]">
                {checkResult?.status === "no_published_release"
                  ? "Aún no hay releases publicados en este canal"
                  : "Atenea está al día"}
              </p>
              <p className="mt-1 text-[11px] text-[#6b7280]">
                {checkResult?.message ??
                  `Versión instalada v${settings?.current_version ?? "0.1.0"}.`}
              </p>
              {checkResult && checkResult.endpoints.length > 0 && (
                <p className="mt-2 truncate font-mono text-[10px] text-[#9ca3af]" title={checkResult.endpoints[0]}>
                  {checkResult.endpoints[0]}
                </p>
              )}
            </div>
          )}

          {errorMsg && (
            <div className="flex items-start gap-2 rounded-xl border border-[#f3d2d5] bg-[#fffafa] p-3 text-xs text-[#991b1b]">
              <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-[#dc2626]" />
              <span className="break-words">{errorMsg}</span>
            </div>
          )}
        </div>

        {/* Pie de acciones */}
        <div className="flex items-center justify-between border-t border-[#eceef1] bg-[#f8f9fa] px-5 py-3">
          <button
            type="button"
            disabled={checking || installing}
            onClick={() => runCheck()}
            className="inline-flex items-center gap-1.5 rounded-lg border border-[#e2e5e9] bg-white px-3 py-1.5 text-xs font-medium text-[#374151] hover:bg-[#f3f4f6] disabled:opacity-40"
          >
            <RefreshCw className={`h-3.5 w-3.5 ${checking ? "animate-spin" : ""}`} />
            Buscar ahora
          </button>

          <div className="flex items-center gap-2">
            <button
              type="button"
              disabled={installing}
              onClick={onClose}
              className="rounded-lg px-3 py-1.5 text-xs text-[#6b7280] hover:bg-[#eceef1] hover:text-[#18181b] disabled:opacity-40"
            >
              {activeUpdate ? "Aún no guardé · Posponer" : "Cerrar"}
            </button>

            {activeUpdate && (
              <button
                type="button"
                disabled={!confirmedSavedWork || installing}
                onClick={handleInstallNow}
                className="inline-flex items-center gap-1.5 rounded-lg bg-[#18181b] px-3.5 py-1.5 text-xs font-medium text-white transition hover:bg-[#27272a] disabled:cursor-not-allowed disabled:opacity-40"
              >
                <Download className="h-3.5 w-3.5" />
                {installing
                  ? "Actualizando…"
                  : `Actualizar a v${activeUpdate.version}`}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
