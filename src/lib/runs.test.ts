import { describe, expect, it } from "vitest";
import type { EntryProgress, RestoreRun } from "@/types";
import {
  MAX_FINISHED_RUNS,
  applyRestoreEvent,
  isActiveEntryStatus,
  isTerminalEntryStatus,
  pruneRuns,
  reconcileActiveRuns,
  runProgress,
  runningRuns,
  type RunsMap,
} from "./runs";

function entry(id: string, status: EntryProgress["status"], name = id): EntryProgress {
  return {
    entryId: id,
    displayName: name,
    appId: id,
    status,
    message: null,
    attempt: 0,
    windowsExpected: 1,
    windowsRestored: 0,
    startedAt: null,
    finishedAt: null,
  };
}

function run(id: string, overrides: Partial<RestoreRun> = {}): RestoreRun {
  return {
    id,
    workspaceId: `ws-${id}`,
    workspaceName: `Workspace ${id}`,
    mode: "merge",
    status: "running",
    startedAt: "2026-11-20T10:00:00Z",
    finishedAt: null,
    entries: [entry("a", "pending"), entry("b", "pending")],
    closedWindows: [],
    messages: [],
    ...overrides,
  };
}

const empty: RunsMap = new Map();

describe("status helpers", () => {
  it("classifies entry statuses", () => {
    expect(isTerminalEntryStatus("completed")).toBe(true);
    expect(isTerminalEntryStatus("skipped")).toBe(true);
    expect(isTerminalEntryStatus("launching")).toBe(false);
    expect(isActiveEntryStatus("restoring")).toBe(true);
    expect(isActiveEntryStatus("pending")).toBe(false);
  });
});

describe("applyRestoreEvent", () => {
  it("adds started runs and replaces them when finished", () => {
    const started = applyRestoreEvent(empty, { type: "runStarted", run: run("r1") });
    expect(started.get("r1")?.status).toBe("running");
    const finished = applyRestoreEvent(started, { type: "runFinished", run: run("r1", { status: "completed" }) });
    expect(finished.get("r1")?.status).toBe("completed");
  });

  it("updates and inserts entries of a running run", () => {
    const runs = applyRestoreEvent(empty, { type: "runStarted", run: run("r1") });
    const updated = applyRestoreEvent(runs, { type: "entryUpdated", runId: "r1", entry: entry("a", "launching") });
    expect(updated.get("r1")?.entries.map((e) => e.status)).toEqual(["launching", "pending"]);
    const inserted = applyRestoreEvent(updated, { type: "entryUpdated", runId: "r1", entry: entry("c", "pending") });
    expect(inserted.get("r1")?.entries).toHaveLength(3);
    // The previous map is not mutated.
    expect(runs.get("r1")?.entries[0].status).toBe("pending");
  });

  it("ignores entry events for unknown or finished runs", () => {
    expect(applyRestoreEvent(empty, { type: "entryUpdated", runId: "x", entry: entry("a", "failed") })).toBe(empty);
    const finished = applyRestoreEvent(empty, { type: "runFinished", run: run("r1", { status: "failed" }) });
    expect(applyRestoreEvent(finished, { type: "entryUpdated", runId: "r1", entry: entry("a", "launching") })).toBe(finished);
  });

  it("never resurrects a finished run with a late runStarted", () => {
    const finished = applyRestoreEvent(empty, { type: "runFinished", run: run("r1", { status: "completed" }) });
    const late = applyRestoreEvent(finished, { type: "runStarted", run: run("r1") });
    expect(late.get("r1")?.status).toBe("completed");
  });

  it("tracks window close outcomes", () => {
    const runs = applyRestoreEvent(empty, { type: "runStarted", run: run("r1") });
    const outcome = { windowId: "0x1", title: "Mail", appName: "Mail", status: "requested" as const, message: null };
    const requested = applyRestoreEvent(runs, { type: "windowCloseUpdated", runId: "r1", outcome });
    const closed = applyRestoreEvent(requested, {
      type: "windowCloseUpdated",
      runId: "r1",
      outcome: { ...outcome, status: "closed" },
    });
    expect(closed.get("r1")?.closedWindows).toEqual([{ ...outcome, status: "closed" }]);
  });
});

describe("reconcileActiveRuns", () => {
  it("adds snapshot runs and drops runs that ended while unobserved", () => {
    let runs: RunsMap = new Map([
      ["gone", run("gone")],
      ["done", run("done", { status: "completed" })],
    ]);
    runs = reconcileActiveRuns(runs, [run("new")]);
    expect([...runs.keys()].sort()).toEqual(["done", "new"]);
  });

  it("does not overwrite a run we already know has finished", () => {
    const runs: RunsMap = new Map([["r1", run("r1", { status: "failed" })]]);
    expect(reconcileActiveRuns(runs, [run("r1")]).get("r1")?.status).toBe("failed");
  });
});

describe("pruneRuns", () => {
  it("keeps every running run and the newest finished ones", () => {
    const entries: Array<[string, RestoreRun]> = [["live", run("live")]];
    for (let i = 0; i < MAX_FINISHED_RUNS + 5; i++) {
      const finishedAt = new Date(Date.UTC(2026, 10, 20, 10, i)).toISOString();
      entries.push([`f${i}`, run(`f${i}`, { status: "completed", finishedAt })]);
    }
    const pruned = pruneRuns(new Map(entries));
    expect(pruned.size).toBe(MAX_FINISHED_RUNS + 1);
    expect(pruned.has("live")).toBe(true);
    expect(pruned.has("f0")).toBe(false);
    expect(pruned.has(`f${MAX_FINISHED_RUNS + 4}`)).toBe(true);
  });
});

describe("runProgress / runningRuns", () => {
  it("counts finished entries and finds the current one", () => {
    const progress = runProgress(
      run("r1", { entries: [entry("a", "completed"), entry("b", "launching", "Firefox"), entry("c", "pending")] }),
    );
    expect(progress).toMatchObject({ done: 1, total: 3 });
    expect(progress.fraction).toBeCloseTo(1 / 3);
    expect(progress.current?.displayName).toBe("Firefox");
  });

  it("is complete when the run is over and empty-safe while running", () => {
    expect(runProgress(run("r1", { status: "failed", entries: [] })).fraction).toBe(1);
    expect(runProgress(run("r1", { entries: [] })).fraction).toBe(0);
  });

  it("lists running runs oldest first, comparing real timestamps", () => {
    const runs: RunsMap = new Map([
      ["b", run("b", { startedAt: "2026-11-20T10:00:00.5Z" })],
      ["a", run("a", { startedAt: "2026-11-20T10:00:00Z" })],
      ["c", run("c", { status: "completed" })],
    ]);
    expect(runningRuns(runs).map((r) => r.id)).toEqual(["a", "b"]);
  });
});
