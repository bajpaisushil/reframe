// Pure helpers for one restore run: merging live events into it, counting progress, the
// finished summary and the per-application details shown on the restore page.
// Tested in runState.test.ts.

import { isActiveEntryStatus, isTerminalEntryStatus } from "@/lib/runs";
import { formatList, pluralize } from "@/lib/format";
import type { CloseStatus, EntryProgress, IconLookup, RestoreEvent, RestoreRun, Workspace } from "@/types";

/** Message the engine gives entries that are turned off in the workspace. */
export const DISABLED_MESSAGE = "Disabled";

/** Retry cap applied by the engine (`MAX_RETRIES` in the orchestrator). */
const MAX_RETRIES = 10;

export function isDisabledEntry(entry: Pick<EntryProgress, "status" | "message">): boolean {
  return entry.status === "skipped" && entry.message === DISABLED_MESSAGE;
}

// ---------------------------------------------------------------------------------------------
// Live events
// ---------------------------------------------------------------------------------------------

/**
 * Apply one `reframe://restore` event to the run shown for `runId`. Returns the same object when
 * the event changes nothing (other runs, or late events for a run that already finished).
 *
 * - `runStarted` / `runFinished` carry the whole run and replace it; a finished run is never
 *   brought back to life by a (late or replayed) start event.
 * - `entryUpdated` / `windowCloseUpdated` patch a running run by entry / window id.
 */
export function applyRunEvent(run: RestoreRun | null, runId: string, event: RestoreEvent): RestoreRun | null {
  switch (event.type) {
    case "runStarted":
      if (event.run.id !== runId) return run;
      if (run && run.status !== "running") return run;
      return event.run;
    case "runFinished":
      return event.run.id === runId ? event.run : run;
    case "entryUpdated": {
      if (event.runId !== runId || !run || run.status !== "running") return run;
      const index = run.entries.findIndex((e) => e.entryId === event.entry.entryId);
      const entries = run.entries.slice();
      if (index === -1) entries.push(event.entry);
      else entries[index] = event.entry;
      return { ...run, entries };
    }
    case "windowCloseUpdated": {
      if (event.runId !== runId || !run || run.status !== "running") return run;
      const index = run.closedWindows.findIndex((c) => c.windowId === event.outcome.windowId);
      const closedWindows = run.closedWindows.slice();
      if (index === -1) closedWindows.push(event.outcome);
      else closedWindows[index] = event.outcome;
      return { ...run, closedWindows };
    }
    default:
      return run;
  }
}

/**
 * Replay events that arrived while the run was loading on top of the loaded copy. Events for each
 * application arrive in order and every update carries its full state, so replaying everything
 * received since subscribing ends with the latest state even if the loaded copy is newer.
 */
export function replayRunEvents(run: RestoreRun | null, runId: string, events: readonly RestoreEvent[]): RestoreRun | null {
  return events.reduce<RestoreRun | null>((current, event) => applyRunEvent(current, runId, event), run);
}

/**
 * A run the database still lists as running although no restore is in progress (Reframe quit or
 * crashed in the middle). Runs younger than `graceMs` are given time to show up as active.
 */
export function isInterruptedRun(
  run: Pick<RestoreRun, "id" | "status" | "startedAt">,
  activeRunIds: ReadonlySet<string>,
  activeKnown: boolean,
  now: number,
  graceMs = 15_000,
): boolean {
  if (run.status !== "running" || !activeKnown || activeRunIds.has(run.id)) return false;
  const started = Date.parse(run.startedAt);
  return Number.isNaN(started) || now - started > graceMs;
}

// ---------------------------------------------------------------------------------------------
// Progress & summary
// ---------------------------------------------------------------------------------------------

export interface RunCounts {
  /** Entries Reframe tries to restore (turned-off entries are not counted). */
  total: number;
  /** Entries that reached a final status. */
  done: number;
  /** Completed or partly completed (what the activity feed calls "restored"). */
  restored: number;
  completed: number;
  partial: number;
  failed: number;
  /** Skipped for a reason other than being turned off (dependency failed, cancelled...). */
  skipped: number;
  /** Turned off in the workspace. */
  disabled: number;
}

export function runCounts(run: Pick<RestoreRun, "entries">): RunCounts {
  const counts: RunCounts = { total: 0, done: 0, restored: 0, completed: 0, partial: 0, failed: 0, skipped: 0, disabled: 0 };
  for (const entry of run.entries) {
    if (isDisabledEntry(entry)) {
      counts.disabled += 1;
      continue;
    }
    counts.total += 1;
    if (isTerminalEntryStatus(entry.status)) counts.done += 1;
    if (entry.status === "completed") counts.completed += 1;
    if (entry.status === "partiallyCompleted") counts.partial += 1;
    if (entry.status === "failed") counts.failed += 1;
    if (entry.status === "skipped") counts.skipped += 1;
  }
  counts.restored = counts.completed + counts.partial;
  return counts;
}

/**
 * Entries worth retrying: failed, partly restored, or skipped because something they depend on
 * failed or the restore was cancelled. Mirrors `RestoreRun::retryable_entries` in the engine.
 */
export function retryableEntryIds(run: Pick<RestoreRun, "entries">): string[] {
  return run.entries
    .filter((e) => {
      if (e.status === "failed" || e.status === "partiallyCompleted") return true;
      if (e.status !== "skipped" || !e.message) return false;
      return e.message.startsWith("Waiting on") || e.message.includes("dependency") || e.message.includes("cancelled");
    })
    .map((e) => e.entryId);
}

export type SummaryTone = "success" | "warning" | "danger" | "neutral";

export interface RunSummary {
  title: string;
  detail: string;
  tone: SummaryTone;
}

function appCount(n: number): string {
  return pluralize(n, "app");
}

/** Headline for a finished run ("Restored 5 of 6 apps") and a short explanation. */
export function runSummary(run: Pick<RestoreRun, "status" | "workspaceName" | "entries" | "messages" | "closedWindows">): RunSummary {
  const counts = runCounts(run);
  const { total, restored } = counts;
  const restoredOf = `Restored ${restored} of ${appCount(total)}`;
  const closing = closingNote(run.closedWindows);
  const withClosing = (text: string) => (closing ? `${text} ${closing}` : text);
  switch (run.status) {
    case "completed":
      if (total === 0) {
        return { title: "Nothing to restore", detail: withClosing("No app in this restore was turned on."), tone: "neutral" };
      }
      return {
        title: restored === total ? (total === 1 ? "Restored 1 app" : `Restored all ${total} apps`) : restoredOf,
        detail: withClosing("Everything is back where you left it."),
        tone: "success",
      };
    case "partiallyCompleted": {
      // Partly restored apps count as restored, so name them instead of "2 of 2".
      const onlyPartial = counts.failed + counts.skipped === 0;
      const title = onlyPartial
        ? total === 1
          ? "Partly restored 1 app"
          : `Restored ${appCount(total)}, ${counts.partial} only partly`
        : restoredOf;
      const detail = onlyPartial
        ? "Some windows couldn't be reopened or arranged. See the details below."
        : `${problemSentence(counts)} See the details below.`;
      return { title, detail: withClosing(detail), tone: "warning" };
    }
    case "failed":
      return {
        title: `Couldn't restore ${run.workspaceName}`,
        detail: total === 0 ? "This restore didn't start." : "None of the apps could be restored. See the details below.",
        tone: "danger",
      };
    case "cancelled":
      return {
        title: "Restore cancelled",
        detail: withClosing(
          restored === 0
            ? "You cancelled before any app opened."
            : `${restored} of ${appCount(total)} had opened before you cancelled — they stay open.`,
        ),
        tone: "neutral",
      };
    case "running":
    default:
      return { title: `Restoring ${run.workspaceName}`, detail: "", tone: "neutral" };
  }
}

/** "1 app couldn't be restored and 1 was skipped." */
function problemSentence(counts: RunCounts): string {
  const parts: string[] = [];
  if (counts.failed > 0) parts.push(`${appCount(counts.failed)} couldn't be restored`);
  if (counts.partial > 0) parts.push(`${appCount(counts.partial)} came back only partly`);
  if (counts.skipped > 0) parts.push(`${counts.skipped} ${counts.skipped === 1 ? "was" : "were"} skipped`);
  const text = formatList(parts);
  return text ? `${text.charAt(0).toUpperCase()}${text.slice(1)}.` : "Some apps need your attention.";
}

/** "Closed 2 windows." / "1 window is still open." for Replace switches. */
function closingNote(outcomes: RestoreRun["closedWindows"]): string {
  if (outcomes.length === 0) return "";
  const closed = outcomes.filter((o) => o.status === "closed").length;
  const open = outcomes.filter((o) => o.status === "stillOpen" || o.status === "failed").length;
  const parts: string[] = [];
  if (closed > 0) parts.push(`Closed ${pluralize(closed, "window")}.`);
  if (open > 0) parts.push(`${pluralize(open, "window")} ${open === 1 ? "is" : "are"} still open.`);
  return parts.join(" ");
}

/** Label for the follow-up button of a finished run. */
export function retryLabel(run: Pick<RestoreRun, "status" | "entries">): string {
  if (run.status === "cancelled") return "Restore the rest";
  return runCounts(run).failed > 0 ? "Retry failed" : "Try again";
}

/** What is happening right now ("Opening Firefox…", "Opening Firefox and 2 more…"). */
export function currentActivity(run: Pick<RestoreRun, "entries" | "closedWindows">): string {
  const active = run.entries.filter((e) => isActiveEntryStatus(e.status));
  if (active.length > 0) {
    const first = active[0];
    const verb = first.status === "validating" ? "Checking" : first.status === "restoring" ? "Arranging" : "Opening";
    const others = active.length - 1;
    return others > 0 ? `${verb} ${first.displayName} and ${others} more…` : `${verb} ${first.displayName}…`;
  }
  if (run.closedWindows.some((c) => c.status === "requested")) return "Asking apps to close…";
  const waiting = run.entries.some((e) => e.status === "pending");
  return waiting ? "Getting ready…" : "Finishing up…";
}

// ---------------------------------------------------------------------------------------------
// One application
// ---------------------------------------------------------------------------------------------

/** How many launch attempts an entry gets (1 + its retries), when the workspace is known. */
export function maxAttemptsFor(workspace: Pick<Workspace, "applications" | "restore"> | null | undefined, entryId: string): number | null {
  if (!workspace) return null;
  const entry = workspace.applications.find((a) => a.id === entryId);
  if (!entry) return null;
  const retries = Math.min(entry.options.retryCount ?? workspace.restore.retryCount, MAX_RETRIES);
  return Math.max(0, retries) + 1;
}

/** "Attempt 2 of 3" once an app is retried (first attempts are not worth mentioning). */
export function attemptLabel(attempt: number, maxAttempts: number | null): string | null {
  if (attempt < 2) return null;
  return maxAttempts !== null && maxAttempts >= attempt ? `Attempt ${attempt} of ${maxAttempts}` : `Attempt ${attempt}`;
}

/** "2 of 3 windows" while arranging and once done (nothing for apps without saved windows). */
export function windowsLabel(entry: Pick<EntryProgress, "status" | "windowsExpected" | "windowsRestored">): string | null {
  const { status, windowsExpected: expected, windowsRestored: restored } = entry;
  if (expected <= 0) return null;
  if (status !== "restoring" && status !== "completed" && status !== "partiallyCompleted") return null;
  return `${Math.min(restored, expected)} of ${expected} ${expected === 1 ? "window" : "windows"}`;
}

/** How long a finished app took (ms), or null. */
export function entryDurationMs(entry: Pick<EntryProgress, "startedAt" | "finishedAt">): number | null {
  if (!entry.startedAt || !entry.finishedAt) return null;
  const ms = Date.parse(entry.finishedAt) - Date.parse(entry.startedAt);
  return Number.isFinite(ms) && ms >= 0 ? ms : null;
}

/** Time since the run started (until it finished). */
export function runElapsedMs(run: Pick<RestoreRun, "startedAt" | "finishedAt">, now: number): number {
  const start = Date.parse(run.startedAt);
  if (Number.isNaN(start)) return 0;
  const end = run.finishedAt ? Date.parse(run.finishedAt) : now;
  return Math.max(0, (Number.isNaN(end) ? now : end) - start);
}

/** Stopwatch format for a live timer: "0:07", "12:30", "1:02:03". */
export function formatClock(ms: number): string {
  const total = Math.max(0, Math.floor((Number.isFinite(ms) ? ms : 0) / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  const ss = String(seconds).padStart(2, "0");
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${ss}` : `${minutes}:${ss}`;
}

/** Engine messages use " - " as a dash; show a real one. "Disabled" gets a friendlier phrase. */
export function displayMessage(message: string | null | undefined): string | null {
  if (!message) return null;
  if (message === DISABLED_MESSAGE) return "Turned off in this workspace";
  return message.replace(/ - /g, " — ");
}

/** Secondary line for an app row: message, attempt and windows, joined with " · ". */
export function entryDetail(entry: EntryProgress, maxAttempts: number | null): string {
  const message = displayMessage(entry.message);
  // Skip the window count when the engine's message already gives one ("1 of 2 windows reopened").
  const windows = message && /\d+ of \d+ windows?/i.test(message) ? null : windowsLabel(entry);
  const parts = [attemptLabel(entry.attempt, maxAttempts), message, windows];
  return parts.filter((p): p is string => !!p).join(" · ");
}

/** Icon lookup for an app in a run: the workspace's launch info when available. */
export function entryIconLookup(workspace: Pick<Workspace, "applications"> | null | undefined, entry: Pick<EntryProgress, "entryId" | "appId">): IconLookup {
  const saved = workspace?.applications.find((a) => a.id === entry.entryId);
  return {
    appId: entry.appId,
    executablePath: saved?.launch.executable ?? null,
    package: saved?.launch.package ?? null,
  };
}

// ---------------------------------------------------------------------------------------------
// Closing windows (Replace)
// ---------------------------------------------------------------------------------------------

export interface CloseStatusCopy {
  label: string;
  tone: "accent" | "success" | "warning" | "danger";
}

export const CLOSE_STATUS_COPY: Record<CloseStatus, CloseStatusCopy> = {
  requested: { label: "Asking to close", tone: "accent" },
  closed: { label: "Closed", tone: "success" },
  stillOpen: { label: "Still open", tone: "warning" },
  failed: { label: "Couldn't close", tone: "danger" },
};

export const STILL_OPEN_GUIDANCE = "Waiting for you — it may be asking to save changes.";
