import React, { useState } from "react";
import { FolderOpen, GitBranch, Trash2, X } from "lucide-react";
import type { CommitItem, WatchedFolder } from "../types";

interface AddFolderModalProps {
  open: boolean;
  onClose: () => void;
  onBrowseNative: () => Promise<string | null>;
  onConfirmPath: (path: string) => Promise<void>;
}

export const AddFolderModal: React.FC<AddFolderModalProps> = ({
  open,
  onClose,
  onBrowseNative,
  onConfirmPath,
}) => {
  const [pathInput, setPathInput] = useState("");
  const [submitting, setSubmitting] = useState(false);

  if (!open) return null;

  const handleBrowse = async () => {
    const picked = await onBrowseNative();
    if (picked) {
      setPathInput(picked);
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!pathInput.trim()) return;
    setSubmitting(true);
    try {
      await onConfirmPath(pathInput.trim());
      setPathInput("");
      onClose();
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/20 p-4 backdrop-blur-[1px]">
      <div className="w-full max-w-md rounded-2xl border border-[#e6e8ec] bg-white p-5 shadow-xl">
        <div className="flex items-center justify-between">
          <h2 className="text-[13.5px] font-semibold text-[#18181b]">
            Vigilar carpeta de proyectos
          </h2>
          <button
            type="button"
            onClick={onClose}
            className="rounded-md p-1 text-[#8a8f98] hover:bg-[#f3f4f6] hover:text-[#18181b]"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <p className="mt-1.5 text-xs leading-relaxed text-[#6b7280]">
          Atenea detectará los archivos{" "}
          <code className="rounded bg-[#f3f4f6] px-1 py-0.5 text-[#374151]">
            .clip
          </code>{" "}
          dentro de la carpeta y registrará cada guardado automáticamente.
        </p>

        <form onSubmit={handleSubmit} className="mt-4 space-y-4">
          <div className="flex gap-2">
            <input
              type="text"
              value={pathInput}
              onChange={(e) => setPathInput(e.target.value)}
              placeholder="C:\Users\...\Mis Dibujos"
              className="flex-1 rounded-lg border border-[#e2e5e9] bg-[#f8f9fa] px-3 py-1.5 text-xs text-[#18181b] placeholder:text-[#9ca3af] focus:border-[#9ca3af] focus:bg-white focus:outline-none"
              autoFocus
            />
            <button
              type="button"
              onClick={handleBrowse}
              className="flex shrink-0 items-center gap-1.5 rounded-lg border border-[#e2e5e9] bg-white px-3 py-1.5 text-xs font-medium text-[#18181b] hover:bg-[#f6f7f9]"
            >
              <FolderOpen className="h-3.5 w-3.5 text-[#5f6368]" />
              Examinar
            </button>
          </div>

          <div className="flex justify-end gap-2 pt-1">
            <button
              type="button"
              onClick={onClose}
              className="rounded-lg px-3 py-1.5 text-xs text-[#6b7280] hover:bg-[#f3f4f6] hover:text-[#18181b]"
            >
              Cancelar
            </button>
            <button
              type="submit"
              disabled={!pathInput.trim() || submitting}
              className="rounded-lg bg-[#18181b] px-3.5 py-1.5 text-xs font-medium text-white hover:bg-[#27272a] disabled:opacity-40"
            >
              {submitting ? "Agregando…" : "Vigilar carpeta"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
};

interface RemoveFolderModalProps {
  folder: WatchedFolder | null;
  projectCountInFolder: number;
  onClose: () => void;
  onConfirmRemove: (folder: WatchedFolder, purgeSnapshots: boolean) => Promise<void>;
}

export const RemoveFolderModal: React.FC<RemoveFolderModalProps> = ({
  folder,
  projectCountInFolder,
  onClose,
  onConfirmRemove,
}) => {
  const [submitting, setSubmitting] = useState(false);

  if (!folder) return null;

  const trigger = async (purge: boolean) => {
    setSubmitting(true);
    try {
      await onConfirmRemove(folder, purge);
      onClose();
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/20 p-4 backdrop-blur-[1px]">
      <div className="w-full max-w-md rounded-2xl border border-[#e6e8ec] bg-white p-5 shadow-xl">
        <div className="flex items-center justify-between">
          <h2 className="text-[13.5px] font-semibold text-[#18181b]">
            Quitar carpeta vigilada
          </h2>
          <button
            type="button"
            onClick={onClose}
            className="rounded-md p-1 text-[#8a8f98] hover:bg-[#f3f4f6] hover:text-[#18181b]"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <p className="mt-2 rounded-lg border border-[#eceef1] bg-[#f8f9fa] px-3 py-1.5 font-mono text-[11px] text-[#4b5563] break-all">
          {folder.path}
        </p>

        <p className="mt-2.5 text-xs leading-relaxed text-[#6b7280]">
          Tus archivos originales nunca se modifican. Elige qué hacer con el historial guardado en Atenea ({projectCountInFolder} obras):
        </p>

        <div className="mt-3.5 space-y-2">
          <button
            type="button"
            disabled={submitting}
            onClick={() => trigger(false)}
            className="flex w-full flex-col rounded-xl border border-[#e2e5e9] bg-white p-3 text-left transition hover:bg-[#f8f9fa] disabled:opacity-50"
          >
            <span className="text-xs font-medium text-[#18181b]">
              Solo dejar de vigilar
            </span>
            <span className="mt-0.5 text-[11px] text-[#6b7280]">
              Pausa el monitoreo pero conserva todos los snapshots existentes.
            </span>
          </button>

          <button
            type="button"
            disabled={submitting}
            onClick={() => trigger(true)}
            className="flex w-full flex-col rounded-xl border border-[#f3d2d5] bg-[#fffafa] p-3 text-left transition hover:bg-[#fef2f2] disabled:opacity-50"
          >
            <span className="flex items-center gap-1.5 text-xs font-medium text-[#b91c1c]">
              <Trash2 className="h-3.5 w-3.5" />
              Quitar y liberar snapshots del disco
            </span>
            <span className="mt-0.5 text-[11px] text-[#6b7280]">
              Elimina el historial interno de estas {projectCountInFolder} obras y recupera espacio.
            </span>
          </button>
        </div>

        <div className="mt-3.5 flex justify-end">
          <button
            type="button"
            onClick={onClose}
            className="rounded-lg px-3 py-1.5 text-xs text-[#6b7280] hover:bg-[#f3f4f6] hover:text-[#18181b]"
          >
            Cancelar
          </button>
        </div>
      </div>
    </div>
  );
};

interface CreateVariationModalProps {
  open: boolean;
  fromCommit: CommitItem | null;
  onClose: () => void;
  onConfirm: (name: string, checkoutToDisk: boolean) => Promise<void>;
}

export const CreateVariationModal: React.FC<CreateVariationModalProps> = ({
  open,
  fromCommit,
  onClose,
  onConfirm,
}) => {
  const [name, setName] = useState("");
  const [checkoutToDisk, setCheckoutToDisk] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  if (!open || !fromCommit) return null;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;
    setSubmitting(true);
    try {
      await onConfirm(name.trim(), checkoutToDisk);
      setName("");
      setCheckoutToDisk(false);
      onClose();
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/20 p-4 backdrop-blur-[1px]">
      <div className="w-full max-w-sm rounded-2xl border border-[#e6e8ec] bg-white p-5 shadow-xl">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <GitBranch className="h-4 w-4 text-[#5f6368]" />
            <h2 className="text-[13.5px] font-semibold text-[#18181b]">
              Nueva variación
            </h2>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded-md p-1 text-[#8a8f98] hover:bg-[#f3f4f6] hover:text-[#18181b]"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <p className="mt-1.5 text-xs text-[#6b7280]">
          Ramificar desde{" "}
          <strong className="font-medium text-[#18181b]">
            {fromCommit.label || `snapshot ${fromCommit.id.slice(0, 7)}`}
          </strong>
          .
        </p>

        <form onSubmit={handleSubmit} className="mt-3.5 space-y-3">
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Ej: Paleta nocturna, Fondo B…"
            className="w-full rounded-lg border border-[#e2e5e9] bg-[#f8f9fa] px-3 py-1.5 text-xs text-[#18181b] placeholder:text-[#9ca3af] focus:border-[#9ca3af] focus:bg-white focus:outline-none"
            autoFocus
          />

          <label className="flex cursor-pointer items-start gap-2 rounded-lg border border-[#eceef1] bg-[#f8f9fa] p-2.5 text-[11px] text-[#5f6368]">
            <input
              type="checkbox"
              checked={checkoutToDisk}
              onChange={(e) => setCheckoutToDisk(e.target.checked)}
              className="mt-0.5 accent-[#18181b]"
            />
            <span>
              Escribir este punto en el archivo <code>.clip</code> ahora (con Clip Studio cerrado).
            </span>
          </label>

          <div className="flex justify-end gap-2 pt-1">
            <button
              type="button"
              onClick={onClose}
              className="rounded-lg px-3 py-1.5 text-xs text-[#6b7280] hover:bg-[#f3f4f6] hover:text-[#18181b]"
            >
              Cancelar
            </button>
            <button
              type="submit"
              disabled={!name.trim() || submitting}
              className="rounded-lg bg-[#18181b] px-3.5 py-1.5 text-xs font-medium text-white hover:bg-[#27272a] disabled:opacity-40"
            >
              {submitting ? "Creando…" : "Crear"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
};
