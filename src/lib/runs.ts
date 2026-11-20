// Pure helpers for live restore runs: applying `reframe://restore` events to a map of runs and
// summarising progress. Used by src/hooks/useActiveRuns.ts (tested in runs.test.ts).

import type { EntryProgress, EntryStatus, RestoreEvent, RestoreRun } from "@/types";

export type RunsMap = ReadonlyMap<string, RestoreRun>;

/** Statuses after which an entry will not change again in this run. */
const TERMINAL_ENTRY: ReadonlySet<EntryStatus> = new Set(["completed", "partiallyCompleted", "failed", "skipped"]);

/** Statuses that mean "working on it" (spinner in the UI). */
const ACTIVE_ENTRY: ReadonlySet<EntryStatus> = new Set(["validating", "launching", "restoring"]);

export function isTerminalEntryStatus(status: EntryStatus): boolean {
  return TERMINAL_ENTRY.has(status);
}

export function isActiveEntryStatus(status: EntryStatus): boolean {
  return ACTIVE_ENTRY.has(status);
}

/** Maximum finished runs kept in memory (running runs are always kept). */
export const MAX_FINISHED_RUNS = 20;

/** Epoch ms of an ISO timestamp (0 when missing/invalid) — RFC 3339 strings with varying
 * fractional digits do not sort correctly as text. */
function time(value: string | null | undefined): number {
  if (!value) return 0;
  const ms = Date.parse(value);
  return Number.isNaN(ms) ? 0 : ms;
}

function upsertEntry(entries: readonly EntryProgress[], entry: EntryProgress): EntryProgress[] {
  const index = entries.findIndex((e) => e.entryId === entry.entryId);
  if (index === -1) return [...entries, entry];
  const next = entries.slice();
  next[index] = entry;
  return next;
}

/** Keep every running run plus the most recent finished ones. */
export function pruneRuns(runs: RunsMap, maxFinished = MAX_FINISHED_RUNS): RunsMap {
  const finished = [...runs.values()].filter((r) => r.status !== "running");
  if (finished.length <= maxFinished) return runs;
  const keep = new Set(
    finished
      .sort((a, b) => time(b.finishedAt ?? b.startedAt) - time(a.finishedAt ?? a.startedAt))
      .slice(0, maxFinished)
      .map((r) => r.id),
  );
  return new Map([...runs].filter(([id, run]) => run.status === "running" || keep.has(id)));
}

/**
 * Apply one event. Returns the same map when nothing changed, otherwise a new map.
 * - runStarted / runFinished carry the full run and replace it.
 * - entryUpdated / windowCloseUpdated patch a known run; events for runs we have never seen are
 *   ignored (the next full snapshot will include them).
 * - A finished run is never resurrected by late entry events.
 */
export function applyRestoreEvent(runs: RunsMap, event: RestoreEvent): RunsMap {
  switch (event.type) {
    case "runStarted":
    case "runFinished": {
      const existing = runs.get(event.run.id);
      if (event.type === "runStarted" && existing && existing.status !== "running") return runs;
      const next = new Map(runs);
      next.set(event.run.id, event.run);
      return pruneRuns(next);
    }
    case "entryUpdated": {
      const run = runs.get(event.runId);
      if (!run || run.status !== "running") return runs;
      const next = new Map(runs);
      next.set(run.id, { ...run, entries: upsertEntry(run.entries, event.entry) });
      return next;
    }
    case "windowCloseUpdated": {
      const run = runs.get(event.runId);
      if (!run || run.status !== "running") return runs;
      const index = run.closedWindows.findIndex((c) => c.windowId === event.outcome.windowId);
      const closedWindows = run.closedWindows.slice();
      if (index === -1) closedWindows.push(event.outcome);
      else closedWindows[index] = event.outcome;
      const next = new Map(runs);
      next.set(run.id, { ...run, closedWindows });
      return next;
    }
    default:
      return runs;
  }
}

/**
 * Reconcile with a fresh `activeRuns()` snapshot: runs in the snapshot are authoritative, and
 * runs we still believe are running but which the backend no longer lists have finished while we
 * were not listening, so they are dropped (their final state is in the restore history).
 */
export function reconcileActiveRuns(runs: RunsMap, active: readonly RestoreRun[]): RunsMap {
  const next = new Map<string, RestoreRun>();
  const activeIds = new Set(active.map((r) => r.id));
  for (const [id, run] of runs) {
    if (run.status === "running" && !activeIds.has(id)) continue;
    next.set(id, run);
  }
  for (const run of active) {
    const known = next.get(run.id);
    if (known && known.status !== "running") continue;
    next.set(run.id, run);
  }
  return pruneRuns(next);
}

export interface RunProgress {
  /** Entries that reached a final status. */
  done: number;
  total: number;
  /** 0..1 */
  fraction: number;
  /** Entry currently being worked on, for "Launching Firefox…" style labels. */
  current: EntryProgress | null;
}

export function runProgress(run: RestoreRun): RunProgress {
  const total = run.entries.length;
  const done = run.entries.filter((e) => isTerminalEntryStatus(e.status)).length;
  const current = run.entries.find((e) => isActiveEntryStatus(e.status)) ?? null;
  const fraction = run.status !== "running" ? 1 : total === 0 ? 0 : done / total;
  return { done, total, fraction, current };
}

/** Running runs, oldest first (stable order for banners). */
export function runningRuns(runs: RunsMap): RestoreRun[] {
  return [...runs.values()].filter((r) => r.status === "running").sort((a, b) => time(a.startedAt) - time(b.startedAt));
}
