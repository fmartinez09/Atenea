import React, { useRef, useState } from "react";
import { ArrowLeftRight, X } from "lucide-react";
import type { CommitItem, VisualDiffReport } from "../types";
import { displayBranchLabel } from "./GitKrakenGraph";

export type ViewMode = "single" | "split" | "onion" | "heatmap";

interface VisualDiffStageProps {
  selectedCommit: CommitItem;
  baseCommit: CommitItem | null;
  diffReport: VisualDiffReport | null;
  loadingDiff: boolean;
  onSwapComparison: () => void;
  onClearComparison: () => void;
}

export const VisualDiffStage: React.FC<VisualDiffStageProps> = ({
  selectedCommit,
  baseCommit,
  diffReport,
  loadingDiff,
  onSwapComparison,
  onClearComparison,
}) => {
  const [mode, setMode] = useState<ViewMode>("split");
  const [splitPct, setSplitPct] = useState<number>(50);
  const [onionOpacity, setOnionOpacity] = useState<number>(55);
  const containerRef = useRef<HTMLDivElement | null>(null);

  const canCompare =
    Boolean(baseCommit) &&
    baseCommit?.id !== selectedCommit.id &&
    Boolean(selectedCommit.preview_data_url);

  const activeMode: ViewMode = canCompare ? mode : "single";

  const handlePointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (activeMode !== "split" || (e.buttons & 1) === 0 || !containerRef.current) {
      return;
    }
    const rect = containerRef.current.getBoundingClientRect();
    const pct = ((e.clientX - rect.left) / Math.max(rect.width, 1)) * 100;
    setSplitPct(Math.max(2, Math.min(98, pct)));
  };

  const handleStageClick = (e: React.MouseEvent<HTMLDivElement>) => {
    if (activeMode !== "split" || !containerRef.current) return;
    const rect = containerRef.current.getBoundingClientRect();
    const pct = ((e.clientX - rect.left) / Math.max(rect.width, 1)) * 100;
    setSplitPct(Math.max(2, Math.min(98, pct)));
  };

  const baseImg =
    diffReport?.base_image_data_url ?? baseCommit?.preview_data_url ?? null;
  const targetImg =
    diffReport?.target_image_data_url ?? selectedCommit.preview_data_url ?? null;

  return (
    <div className="flex h-full flex-col bg-[#fafafa]">
      {/* Barra única, limpia y sin saltos de línea (42px, alineada con las demás columnas) */}
      <div className="flex h-10.5 shrink-0 items-center justify-between gap-3 border-b border-[#eceef1] bg-white px-4">
        {/* Selector segmentado sobrio */}
        <div className="flex shrink-0 items-center rounded-lg bg-[#f3f4f6] p-0.5 text-[12px]">
          {(
            [
              { id: "single", label: "Lienzo", needsCompare: false },
              { id: "split", label: "Cortina", needsCompare: true },
              { id: "onion", label: "Cebolla", needsCompare: true },
              { id: "heatmap", label: "Diferencias", needsCompare: true },
            ] as const
          ).map((tab) => {
            const active = activeMode === tab.id;
            return (
              <button
                key={tab.id}
                type="button"
                disabled={tab.needsCompare && !canCompare}
                onClick={() => setMode(tab.id)}
                className={`rounded-md px-2.5 py-1 font-medium transition disabled:opacity-35 ${
                  active
                    ? "bg-white text-[#18181b] shadow-[0_1px_2px_rgba(0,0,0,0.06)]"
                    : "text-[#6b7280] hover:text-[#18181b]"
                }`}
              >
                {tab.label}
              </button>
            );
          })}
        </div>

        {/* Estado contextual discreto a la derecha */}
        <div className="flex min-w-0 items-center gap-2 text-[12px] text-[#6b7280]">
          {activeMode === "onion" && (
            <label className="flex shrink-0 items-center gap-2">
              <span className="text-[11.5px]">Opacidad</span>
              <input
                type="range"
                min={0}
                max={100}
                value={onionOpacity}
                onChange={(e) => setOnionOpacity(Number(e.target.value))}
                className="w-20 accent-[#18181b]"
              />
              <span className="w-8 text-right font-mono text-[11px] text-[#18181b]">
                {onionOpacity}%
              </span>
            </label>
          )}

          {canCompare && diffReport && !loadingDiff && (
            <span className="shrink-0 rounded-md bg-[#f3f4f6] px-2 py-0.5 font-mono text-[11px] text-[#374151]">
              {(diffReport.changed_ratio * 100).toFixed(1)}% cambiado
            </span>
          )}

          {canCompare && baseCommit && (
            <div className="flex items-center gap-1 border-l border-[#eceef1] pl-2">
              <button
                type="button"
                onClick={onSwapComparison}
                className="rounded p-1 text-[#6b7280] transition hover:bg-[#f3f4f6] hover:text-[#18181b]"
                title="Invertir A y B"
              >
                <ArrowLeftRight className="h-3.5 w-3.5" />
              </button>
              <button
                type="button"
                onClick={onClearComparison}
                className="rounded p-1 text-[#6b7280] transition hover:bg-[#f3f4f6] hover:text-[#18181b]"
                title="Salir de comparación"
              >
                <X className="h-3.5 w-3.5" />
              </button>
            </div>
          )}
        </div>
      </div>

      {/* Escenario despejado estilo estudio */}
      <div className="relative flex flex-1 flex-col items-center justify-center overflow-hidden p-6">
        {!targetImg ? (
          <div className="text-center text-xs text-[#8a8f98]">
            Este archivo .clip no incluye miniatura en CanvasPreview.
          </div>
        ) : activeMode === "single" ? (
          <div className="overflow-hidden rounded-xl border border-[#e6e8ec] bg-white p-2 shadow-[0_6px_24px_rgba(0,0,0,0.04)]">
            <img
              src={targetImg}
              alt="Vista previa"
              className="max-h-[60vh] max-w-full rounded-lg object-contain select-none"
              draggable={false}
            />
          </div>
        ) : activeMode === "split" && baseImg ? (
          <div
            ref={containerRef}
            onPointerMove={handlePointerMove}
            onClick={handleStageClick}
            className="relative inline-block max-h-[60vh] max-w-full cursor-ew-resize overflow-hidden rounded-xl border border-[#e2e5ea] bg-white shadow-[0_6px_24px_rgba(0,0,0,0.05)] select-none"
          >
            <img
              src={targetImg}
              alt="Después"
              className="block max-h-[60vh] max-w-full object-contain"
              draggable={false}
            />

            <div
              className="pointer-events-none absolute inset-0 overflow-hidden"
              style={{ clipPath: `inset(0 ${100 - splitPct}% 0 0)` }}
            >
              <img
                src={baseImg}
                alt="Antes"
                className="block h-full w-full object-contain"
                draggable={false}
              />
            </div>

            <div
              className="pointer-events-none absolute inset-y-0 w-px bg-[#18181b]/75"
              style={{ left: `${splitPct}%` }}
            >
              <div className="absolute top-1/2 -left-3 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded-full border border-[#d4d7dd] bg-white text-[10px] text-[#18181b] shadow-sm">
                ↔
              </div>
            </div>
          </div>
        ) : activeMode === "onion" && baseImg ? (
          <div className="relative inline-block max-h-[60vh] max-w-full overflow-hidden rounded-xl border border-[#e2e5ea] bg-white shadow-[0_6px_24px_rgba(0,0,0,0.05)] select-none">
            <img
              src={baseImg}
              alt="Base"
              className="block max-h-[60vh] max-w-full object-contain"
              draggable={false}
            />
            <img
              src={targetImg}
              alt="Superposición"
              style={{ opacity: onionOpacity / 100 }}
              className="pointer-events-none absolute inset-0 h-full w-full object-contain"
              draggable={false}
            />
          </div>
        ) : (
          <div className="flex flex-col items-center">
            {diffReport?.heatmap_data_url ? (
              <div className="overflow-hidden rounded-xl border border-[#e6e8ec] bg-white p-2 shadow-[0_6px_24px_rgba(0,0,0,0.04)]">
                <img
                  src={diffReport.heatmap_data_url}
                  alt="Mapa de diferencias"
                  className="max-h-[60vh] max-w-full rounded-lg object-contain select-none"
                  draggable={false}
                />
              </div>
            ) : (
              <div className="text-xs text-[#8a8f98]">
                {loadingDiff ? "Calculando diferencias…" : "Sin diferencias visuales."}
              </div>
            )}
          </div>
        )}

        {/* Pie editorial limpio debajo del lienzo (nunca tapando la ilustración) */}
        <div className="mt-3 flex max-w-full items-center gap-2 text-[11.5px] text-[#6b7280]">
          {canCompare && baseCommit ? (
            <>
              <span className="truncate max-w-52">
                <strong className="font-medium text-[#374151]">A:</strong>{" "}
                {baseCommit.label || baseCommit.id.slice(0, 7)}
              </span>
              <span className="text-[#c4c7cc]">→</span>
              <span className="truncate max-w-52">
                <strong className="font-medium text-[#18181b]">B:</strong>{" "}
                {selectedCommit.label || selectedCommit.id.slice(0, 7)}
              </span>
            </>
          ) : (
            <span className="truncate">
              {selectedCommit.label || `Snapshot ${selectedCommit.id.slice(0, 7)}`}{" "}
              <span className="text-[#9ca3af]">
                · {displayBranchLabel(selectedCommit.branch_name)}
              </span>
            </span>
          )}
        </div>
      </div>
    </div>
  );
};
