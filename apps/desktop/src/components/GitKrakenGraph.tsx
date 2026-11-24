import React, { useMemo, useState } from "react";
import {
  Check,
  GitBranch,
  Plus,
  Search,
  Star,
  Tag,
} from "lucide-react";
import type { BranchInfo, CommitItem } from "../types";

// Paleta sobria y editorial (sin colores chillones)
export const LANE_COLORS = [
  "#2563eb", // Principal - Azul editorial limpio
  "#64748b", // Variación 1 - Pizarra
  "#7c3aed", // Variación 2 - Iris sobrio
  "#0d9488", // Variación 3 - Verde azulado tenue
  "#b45309", // Variación 4 - Ocre
  "#be123c", // Variación 5 - Carmesí apagado
];

export function displayBranchLabel(name: string): string {
  return name === "main" ? "Principal" : name;
}

function formatBytesCompact(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const kb = 1024;
  const mb = kb * 1024;
  if (bytes >= mb) return `${(bytes / mb).toFixed(1)} MB`;
  if (bytes >= kb) return `${(bytes / kb).toFixed(0)} KB`;
  return `${bytes} B`;
}

function formatShortTime(ms: number): string {
  return new Date(ms).toLocaleString("es-CL", {
    day: "2-digit",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  });
}

interface GitKrakenGraphProps {
  projectName: string;
  branches: BranchInfo[];
  activeBranch: string;
  commits: CommitItem[];
  selectedCommitId: string | null;
  baseCommitId: string | null;
  busy: boolean;
  onSelectCommit: (commitId: string) => void;
  onSetBaseCommit: (commitId: string) => void;
  onTogglePin: (commitId: string, pinned: boolean) => void;
  onSaveLabel: (commitId: string, label: string | null) => Promise<void>;
  onManualSnapshot: () => void;
  onOpenNewVariation: (fromCommit: CommitItem) => void;
  onSwitchVariation: (branchName: string) => void;
}

const ROW_HEIGHT = 56;
const LANE_SPACING = 16;
const GRAPH_PAD_X = 14;

export const GitKrakenGraph: React.FC<GitKrakenGraphProps> = ({
  projectName,
  branches,
  activeBranch,
  commits,
  selectedCommitId,
  baseCommitId,
  busy,
  onSelectCommit,
  onSetBaseCommit,
  onTogglePin,
  onSaveLabel,
  onManualSnapshot,
  onOpenNewVariation,
  onSwitchVariation,
}) => {
  const [searchQuery, setSearchQuery] = useState("");
  const [onlyPinned, setOnlyPinned] = useState(false);
  const [branchFilter, setBranchFilter] = useState<string>("__ALL__");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draftLabel, setDraftLabel] = useState("");

  const branchLaneMap = useMemo(() => {
    const map = new Map<string, number>();
    map.set("main", 0);
    let next = 1;
    for (const b of branches) {
      if (!map.has(b.name)) {
        map.set(b.name, next++);
      }
    }
    for (const c of commits) {
      if (!map.has(c.branch_name)) {
        map.set(c.branch_name, next++);
      }
    }
    return map;
  }, [branches, commits]);

  const headBranchBadges = useMemo(() => {
    const map = new Map<string, BranchInfo[]>();
    for (const b of branches) {
      if (b.head_commit_id) {
        const arr = map.get(b.head_commit_id) ?? [];
        arr.push(b);
        map.set(b.head_commit_id, arr);
      }
    }
    return map;
  }, [branches]);

  const visibleCommits = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    return commits.filter((c) => {
      if (onlyPinned && !c.pinned) return false;
      if (branchFilter !== "__ALL__" && c.branch_name !== branchFilter) {
        return false;
      }
      if (!q) return true;
      const inLabel = (c.label ?? "").toLowerCase().includes(q);
      const inBranch = displayBranchLabel(c.branch_name).toLowerCase().includes(q);
      const inId = c.id.toLowerCase().includes(q);
      const inLayers = c.metadata.layers.some((l) =>
        l.name.toLowerCase().includes(q)
      );
      return inLabel || inBranch || inId || inLayers;
    });
  }, [branchFilter, commits, onlyPinned, searchQuery]);

  const laneCount = Math.max(1, branchLaneMap.size);
  const svgWidth = Math.max(40, GRAPH_PAD_X * 2 + (laneCount - 1) * LANE_SPACING);

  const rowIndexById = useMemo(() => {
    const m = new Map<string, number>();
    visibleCommits.forEach((c, idx) => m.set(c.id, idx));
    return m;
  }, [visibleCommits]);

  const edges = useMemo(() => {
    const list: Array<{
      key: string;
      x1: number;
      y1: number;
      x2: number;
      y2: number;
      color: string;
    }> = [];

    visibleCommits.forEach((c, idx) => {
      if (!c.parent_id) return;
      const parentIdx = rowIndexById.get(c.parent_id);
      if (parentIdx === undefined) return;

      const childLane = branchLaneMap.get(c.branch_name) ?? 0;
      const parentCommit = visibleCommits[parentIdx];
      const parentLane = branchLaneMap.get(parentCommit.branch_name) ?? 0;

      const x1 = GRAPH_PAD_X + childLane * LANE_SPACING;
      const y1 = idx * ROW_HEIGHT + ROW_HEIGHT / 2;
      const x2 = GRAPH_PAD_X + parentLane * LANE_SPACING;
      const y2 = parentIdx * ROW_HEIGHT + ROW_HEIGHT / 2;
      const color = LANE_COLORS[childLane % LANE_COLORS.length];

      list.push({
        key: `${c.id}->${c.parent_id}`,
        x1,
        y1,
        x2,
        y2,
        color,
      });
    });

    return list;
  }, [branchLaneMap, rowIndexById, visibleCommits]);

  const selectedCommit =
    commits.find((c) => c.id === selectedCommitId) ?? commits[0] ?? null;

  return (
    <section className="flex w-[405px] shrink-0 flex-col border-r border-[#eceef1] bg-white">
      {/* Cabecera estilo miga de pan de Antigravity ("Atenea / Plan Prototipo...") */}
      <div className="flex h-10.5 shrink-0 items-center justify-between border-b border-[#eceef1] px-4">
        <div className="flex min-w-0 items-center gap-1.5 text-[12.5px]">
          <span className="text-[#8a8f98]">Obra</span>
          <span className="text-[#d0d4dc]">/</span>
          <span className="truncate font-medium text-[#18181b]">
            {projectName}
          </span>
        </div>

        <div className="flex shrink-0 items-center gap-1">
          {selectedCommit && (
            <button
              type="button"
              onClick={() => onOpenNewVariation(selectedCommit)}
              className="flex items-center gap-1 rounded-md px-2 py-1 text-[11.5px] text-[#5f6368] transition hover:bg-[#f3f4f6] hover:text-[#18181b]"
              title="Crear variación desde el punto seleccionado"
            >
              <GitBranch className="h-3.5 w-3.5" />
              Variación
            </button>
          )}
          <button
            type="button"
            disabled={busy}
            onClick={onManualSnapshot}
            className="flex items-center gap-1 rounded-md border border-[#e4e6ea] bg-white px-2.5 py-1 text-[11.5px] font-medium text-[#18181b] shadow-[0_1px_1px_rgba(0,0,0,0.02)] transition hover:bg-[#f6f7f9] disabled:opacity-50"
          >
            <Plus className="h-3 w-3" />
            Guardar
          </button>
        </div>
      </div>

      {/* Sub-barra limpia de filtro de variaciones y búsqueda */}
      <div className="border-b border-[#f0f1f4] px-3.5 py-2 space-y-2">
        {branches.length > 0 && (
          <div className="flex flex-wrap items-center gap-1 text-[11.5px]">
            <button
              type="button"
              onClick={() => setBranchFilter("__ALL__")}
              className={`rounded-md px-2 py-0.5 transition ${
                branchFilter === "__ALL__"
                  ? "bg-[#eceef1] font-medium text-[#18181b]"
                  : "text-[#6b7280] hover:bg-[#f5f6f8] hover:text-[#18181b]"
              }`}
            >
              Todas
            </button>

            {branches.map((b) => {
              const lane = branchLaneMap.get(b.name) ?? 0;
              const color = LANE_COLORS[lane % LANE_COLORS.length];
              const isFiltered = branchFilter === b.name;
              const isActive = b.name === activeBranch;
              return (
                <div
                  key={b.name}
                  className={`group flex items-center gap-1 rounded-md px-2 py-0.5 transition ${
                    isFiltered
                      ? "bg-[#eceef1] font-medium text-[#18181b]"
                      : "text-[#5f6368] hover:bg-[#f5f6f8] hover:text-[#18181b]"
                  }`}
                >
                  <button
                    type="button"
                    onClick={() =>
                      setBranchFilter(isFiltered ? "__ALL__" : b.name)
                    }
                    className="flex items-center gap-1.5"
                  >
                    <span
                      className="h-1.5 w-1.5 rounded-full"
                      style={{ backgroundColor: color }}
                    />
                    <span>{displayBranchLabel(b.name)}</span>
                  </button>

                  {isActive ? (
                    <span
                      className="text-[10px] text-[#8a8f98]"
                      title="Variación activa para guardados automáticos"
                    >
                      · activa
                    </span>
                  ) : (
                    <button
                      type="button"
                      onClick={() => onSwitchVariation(b.name)}
                      className="hidden group-hover:inline text-[10px] text-[#2563eb] hover:underline"
                    >
                      activar
                    </button>
                  )}
                </div>
              );
            })}
          </div>
        )}

        <div className="flex items-center gap-1.5">
          <div className="relative flex-1">
            <Search className="pointer-events-none absolute top-1/2 left-2.5 h-3.5 w-3.5 -translate-y-1/2 text-[#9ca3af]" />
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder="Filtrar snapshots…"
              className="w-full rounded-lg border border-transparent bg-[#f4f5f7] py-1.5 pr-2.5 pl-8 text-xs text-[#18181b] placeholder:text-[#9ca3af] transition focus:border-[#d8dce3] focus:bg-white focus:outline-none"
            />
          </div>

          <button
            type="button"
            onClick={() => setOnlyPinned((v) => !v)}
            className={`flex shrink-0 items-center gap-1 rounded-lg px-2 py-1.5 text-[11.5px] transition ${
              onlyPinned
                ? "bg-[#eceef1] font-medium text-[#18181b]"
                : "text-[#6b7280] hover:bg-[#f4f5f7] hover:text-[#18181b]"
            }`}
            title="Filtrar hitos destacados"
          >
            <Star
              className={`h-3.5 w-3.5 ${
                onlyPinned
                  ? "fill-[#18181b] text-[#18181b]"
                  : "text-[#8a8f98]"
              }`}
            />
          </button>
        </div>
      </div>

      {/* Lista de grafo con trazos finos de 1.5px */}
      <div className="relative flex-1 overflow-y-auto select-none">
        {visibleCommits.length === 0 ? (
          <div className="p-8 text-center text-xs text-[#8a8f98]">
            Sin snapshots que coincidan con el filtro.
          </div>
        ) : (
          <div
            className="relative"
            style={{ minHeight: visibleCommits.length * ROW_HEIGHT }}
          >
            <svg
              width={svgWidth}
              height={visibleCommits.length * ROW_HEIGHT}
              className="pointer-events-none absolute top-0 left-0 z-10"
            >
              {edges.map((e) => {
                if (e.x1 === e.x2) {
                  return (
                    <line
                      key={e.key}
                      x1={e.x1}
                      y1={e.y1}
                      x2={e.x2}
                      y2={e.y2}
                      stroke={e.color}
                      strokeWidth={1.5}
                      strokeOpacity={0.55}
                    />
                  );
                }
                const midY = e.y1 + (e.y2 - e.y1) * 0.55;
                const d = `M ${e.x1} ${e.y1} C ${e.x1} ${midY}, ${e.x2} ${midY}, ${e.x2} ${e.y2}`;
                return (
                  <path
                    key={e.key}
                    d={d}
                    fill="none"
                    stroke={e.color}
                    strokeWidth={1.5}
                    strokeOpacity={0.65}
                  />
                );
              })}

              {visibleCommits.map((c, idx) => {
                const lane = branchLaneMap.get(c.branch_name) ?? 0;
                const cx = GRAPH_PAD_X + lane * LANE_SPACING;
                const cy = idx * ROW_HEIGHT + ROW_HEIGHT / 2;
                const color = LANE_COLORS[lane % LANE_COLORS.length];
                const isSelected = c.id === selectedCommitId;
                const isBase = c.id === baseCommitId;
                const isHead = headBranchBadges.has(c.id);

                return (
                  <g key={`node-${c.id}`}>
                    {isSelected && (
                      <circle
                        cx={cx}
                        cy={cy}
                        r={7}
                        fill={color}
                        fillOpacity={0.14}
                      />
                    )}
                    {isBase && !isSelected && (
                      <circle
                        cx={cx}
                        cy={cy}
                        r={6.5}
                        fill="none"
                        stroke="#9ca3af"
                        strokeWidth={1.2}
                        strokeDasharray="2 2"
                      />
                    )}
                    <circle
                      cx={cx}
                      cy={cy}
                      r={isHead ? 4 : 3.4}
                      fill={isSelected || isHead ? color : "#ffffff"}
                      stroke={color}
                      strokeWidth={1.8}
                    />
                  </g>
                );
              })}
            </svg>

            {visibleCommits.map((c, idx) => {
              const isSelected = c.id === selectedCommitId;
              const isBase = c.id === baseCommitId;
              const headBadges = headBranchBadges.get(c.id) ?? [];

              return (
                <div
                  key={c.id}
                  onClick={() => onSelectCommit(c.id)}
                  style={{ height: ROW_HEIGHT, paddingLeft: svgWidth + 4 }}
                  className={`group relative flex cursor-pointer items-center gap-2.5 border-b border-[#f3f4f6] pr-3 transition ${
                    isSelected ? "bg-[#f3f5f8]" : "hover:bg-[#f9fafb]"
                  }`}
                >
                  {/* Miniatura discreta */}
                  <div className="h-9 w-11 shrink-0 overflow-hidden rounded border border-[#e6e8ec] bg-[#f8f9fa]">
                    {c.preview_data_url && (
                      <img
                        src={c.preview_data_url}
                        alt=""
                        className="h-full w-full object-cover"
                      />
                    )}
                  </div>

                  {/* Texto limpio estilo editorial */}
                  <div className="min-w-0 flex-1">
                    {editingId === c.id ? (
                      <div
                        onClick={(e) => e.stopPropagation()}
                        className="flex items-center gap-1"
                      >
                        <input
                          type="text"
                          value={draftLabel}
                          onChange={(e) => setDraftLabel(e.target.value)}
                          placeholder="Nombre del hito…"
                          className="w-full rounded border border-[#cbd5e1] bg-white px-2 py-0.5 text-xs text-[#18181b] focus:outline-none"
                          autoFocus
                          onKeyDown={(e) => {
                            if (e.key === "Enter") {
                              onSaveLabel(c.id, draftLabel.trim() || null);
                              setEditingId(null);
                            }
                            if (e.key === "Escape") setEditingId(null);
                          }}
                        />
                        <button
                          type="button"
                          onClick={() => {
                            onSaveLabel(c.id, draftLabel.trim() || null);
                            setEditingId(null);
                          }}
                          className="rounded bg-[#18181b] p-1 text-white"
                        >
                          <Check className="h-3 w-3" />
                        </button>
                      </div>
                    ) : (
                      <>
                        <div className="flex items-center gap-1.5">
                          <span className="truncate text-[12.5px] font-medium text-[#18181b]">
                            {c.label ||
                              `Snapshot #${visibleCommits.length - idx}`}
                          </span>

                          {headBadges.map((hb) => (
                            <span
                              key={hb.name}
                              className="shrink-0 rounded bg-[#eef0f3] px-1.5 py-0 text-[10px] font-medium text-[#4b5563]"
                            >
                              {displayBranchLabel(hb.name)}
                            </span>
                          ))}
                        </div>

                        <div className="mt-0.5 flex items-center gap-1.5 text-[11px] text-[#8a8f98] tabular-nums">
                          <span>{formatShortTime(c.created_at)}</span>
                          <span>·</span>
                          <span>+{formatBytesCompact(c.added_bytes)}</span>
                          <span>·</span>
                          <span>{c.metadata.layers.length} capas</span>
                        </div>
                      </>
                    )}
                  </div>

                  {/* Controles secundarios minimalistas */}
                  <div
                    onClick={(e) => e.stopPropagation()}
                    className="flex shrink-0 items-center gap-0.5"
                  >
                    <button
                      type="button"
                      onClick={() => onTogglePin(c.id, !c.pinned)}
                      className={`rounded p-1 transition ${
                        c.pinned
                          ? "text-[#18181b]"
                          : "text-[#9ca3af] opacity-0 group-hover:opacity-100 hover:text-[#18181b]"
                      }`}
                      title={c.pinned ? "Quitar hito" : "Marcar hito"}
                    >
                      <Star
                        className={`h-3 w-3 ${
                          c.pinned ? "fill-[#18181b]" : ""
                        }`}
                      />
                    </button>

                    <button
                      type="button"
                      onClick={() => {
                        setEditingId(c.id);
                        setDraftLabel(c.label ?? "");
                      }}
                      className="rounded p-1 text-[#9ca3af] opacity-0 group-hover:opacity-100 hover:bg-[#eceef1] hover:text-[#18181b] transition"
                      title="Renombrar snapshot"
                    >
                      <Tag className="h-3 w-3" />
                    </button>

                    <button
                      type="button"
                      onClick={() => onSetBaseCommit(c.id)}
                      className={`ml-0.5 rounded px-1.5 py-0.5 text-[10.5px] font-medium transition ${
                        isBase
                          ? "bg-[#eceef1] text-[#18181b]"
                          : "text-[#6b7280] opacity-0 group-hover:opacity-100 hover:bg-[#eceef1] hover:text-[#18181b]"
                      }`}
                      title="Usar como punto A de comparación"
                    >
                      {isBase ? "Base A" : "Comparar"}
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </section>
  );
};
