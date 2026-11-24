import React from "react";
import { ArrowUpCircle } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isTauriRuntime } from "../api";
import type { AvailableUpdate } from "../types";

interface TitleBarProps {
  logicalBytes: number;
  physicalBytes: number;
  savingsPct: number;
  snapshotCount: number;
  availableUpdate: AvailableUpdate | null;
  onAddFolder: () => void;
  onSeedDemo: () => void;
  onOpenUpdates: () => void;
}

function formatCompact(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const kb = 1024;
  const mb = kb * 1024;
  const gb = mb * 1024;
  if (bytes >= gb) return `${(bytes / gb).toFixed(2)} GB`;
  if (bytes >= mb) return `${(bytes / mb).toFixed(1)} MB`;
  if (bytes >= kb) return `${(bytes / kb).toFixed(0)} KB`;
  return `${bytes} B`;
}

export const TitleBar: React.FC<TitleBarProps> = ({
  logicalBytes,
  physicalBytes,
  savingsPct,
  snapshotCount,
  availableUpdate,
  onAddFolder,
  onSeedDemo,
  onOpenUpdates,
}) => {
  const handleMinimize = () => {
    if (isTauriRuntime()) getCurrentWindow().minimize();
  };

  const handleToggleMaximize = () => {
    if (isTauriRuntime()) getCurrentWindow().toggleMaximize();
  };

  const handleClose = () => {
    if (isTauriRuntime()) getCurrentWindow().close();
  };

  return (
    <header
      data-tauri-drag-region
      className="flex h-9 shrink-0 items-center justify-between border-b border-[#eceef1] bg-[#f8f9fa] pl-3.5 select-none"
    >
      {/* Menú superior sobrio estilo Antigravity (Antigravity  Workspace  View) */}
      <div
        data-tauri-drag-region
        className="flex items-center gap-4 text-[12px] text-[#5f6368]"
      >
        <span
          data-tauri-drag-region
          className="font-medium text-[#18181b] tracking-tight"
        >
          Atenea
        </span>

        <button
          type="button"
          onClick={onAddFolder}
          className="rounded px-1.5 py-0.5 text-[#5f6368] transition hover:bg-black/[0.04] hover:text-[#18181b]"
        >
          Carpetas
        </button>

        <button
          type="button"
          onClick={onSeedDemo}
          className="rounded px-1.5 py-0.5 text-[#5f6368] transition hover:bg-black/[0.04] hover:text-[#18181b]"
        >
          Cargar demo
        </button>

        <button
          type="button"
          onClick={onOpenUpdates}
          className={
            availableUpdate
              ? "inline-flex items-center gap-1.5 rounded-full bg-[#18181b] px-2.5 py-0.5 text-[11px] font-medium text-white shadow-xs transition hover:bg-[#27272a]"
              : "rounded px-1.5 py-0.5 text-[#5f6368] transition hover:bg-black/[0.04] hover:text-[#18181b]"
          }
          title={
            availableUpdate
              ? `Nueva versión v${availableUpdate.version} lista para instalar`
              : "Buscar actualizaciones o cambiar canal"
          }
        >
          {availableUpdate ? (
            <>
              <ArrowUpCircle className="h-3 w-3 text-[#4ade80]" />
              <span>Actualizar a v{availableUpdate.version}</span>
            </>
          ) : (
            <span>Actualizaciones</span>
          )}
        </button>

        <span
          data-tauri-drag-region
          className="hidden sm:inline-flex items-center gap-1.5 text-[11.5px] text-[#8a8f98]"
        >
          <span className="h-1.5 w-1.5 rounded-full bg-[#16a34a]" />
          Protección activa
          {snapshotCount > 0 && (
            <span className="text-[#9ca3af]">
              · {formatCompact(physicalBytes)} en disco de{" "}
              {formatCompact(logicalBytes)} (−{savingsPct.toFixed(0)}%)
            </span>
          )}
        </span>
      </div>

      {/* Controles nativos limpios de ventana estilo Windows 11 / Antigravity */}
      <div className="flex h-full items-stretch">
        <button
          type="button"
          onClick={handleMinimize}
          aria-label="Minimizar"
          className="flex w-11 items-center justify-center text-[#5f6368] transition hover:bg-black/[0.05] hover:text-[#18181b]"
        >
          <svg width="10" height="1" viewBox="0 0 10 1" fill="none">
            <path d="M0 0.5H10" stroke="currentColor" strokeWidth="1" />
          </svg>
        </button>

        <button
          type="button"
          onClick={handleToggleMaximize}
          aria-label="Maximizar"
          className="flex w-11 items-center justify-center text-[#5f6368] transition hover:bg-black/[0.05] hover:text-[#18181b]"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none">
            <rect
              x="0.5"
              y="0.5"
              width="9"
              height="9"
              rx="1"
              stroke="currentColor"
              strokeWidth="1"
            />
          </svg>
        </button>

        <button
          type="button"
          onClick={handleClose}
          aria-label="Cerrar a bandeja"
          title="Minimizar a la bandeja del sistema"
          className="flex w-11 items-center justify-center text-[#5f6368] transition hover:bg-[#e81123] hover:text-white"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none">
            <path
              d="M1 1L9 9M9 1L1 9"
              stroke="currentColor"
              strokeWidth="1.1"
              strokeLinecap="round"
            />
          </svg>
        </button>
      </div>
    </header>
  );
};
