import { describe, expect, it } from "vitest";
import type { CloseOutcome, EntryProgress, RestoreEvent, RestoreRun, Workspace } from "@/types";
import {
  applyRunEvent,
  attemptLabel,
  currentActivity,
  displayMessage,
  entryDetail,
  entryDurationMs,
  entryIconLookup,
  formatClock,
  isInterruptedRun,
  maxAttemptsFor,
  replayRunEvents,
  retryLabel,
  retryableEntryIds,
  runCounts,
  runElapsedMs,
  runSummary,
  windowsLabel,
} from "./runState";

function entry(id: string, status: EntryProgress["status"], overrides: Partial<EntryProgress> = {}): EntryProgress {
  return {
    entryId: id,
    displayName: id.toUpperCase(),
    appId: `app-${id}`,
    status,
    message: null,
    attempt: 0,
    windowsExpected: 1,
    windowsRestored: 0,
    startedAt: null,
    finishedAt: null,
    ...overrides,
  };
}

function run(overrides: Partial<RestoreRun> = {}): RestoreRun {
  return {
    id: "r1",
    workspaceId: "ws1",
    workspaceName: "Development",
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

function outcome(windowId: string, status: CloseOutcome["status"]): CloseOutcome {
  return { windowId, title: `Window ${windowId}`, appName: "XTerm", status, message: null };
}

describe("applyRunEvent", () => {
  it("patches entries of the run on the page", () => {
    const base = run();
    const next = applyRunEvent(base, "r1", { type: "entryUpdated", runId: "r1", entry: entry("a", "launching", { attempt: 1 }) });
    expect(next?.entries[0].status).toBe("launching");
    expect(next?.entries[1]).toBe(base.entries[1]);
    expect(base.entries[0].status).toBe("pending");
  });

  it("appends entries it has not seen", () => {
    const next = applyRunEvent(run(), "r1", { type: "entryUpdated", runId: "r1", entry: entry("c", "launching") });
    expect(next?.entries.map((e) => e.entryId)).toEqual(["a", "b", "c"]);
  });

  it("ignores events for other runs", () => {
    const base = run();
    expect(applyRunEvent(base, "r1", { type: "entryUpdated", runId: "r2", entry: entry("a", "failed") })).toBe(base);
    expect(applyRunEvent(base, "r1", { type: "runFinished", run: run({ id: "r2", status: "failed" }) })).toBe(base);
    expect(applyRunEvent(base, "r1", { type: "runStarted", run: run({ id: "r2" }) })).toBe(base);
  });

  it("tracks close outcomes by window id", () => {
    const withRequest = applyRunEvent(run(), "r1", { type: "windowCloseUpdated", runId: "r1", outcome: outcome("w1", "requested") });
    const closed = applyRunEvent(withRequest, "r1", { type: "windowCloseUpdated", runId: "r1", outcome: outcome("w1", "closed") });
    expect(closed?.closedWindows).toEqual([outcome("w1", "closed")]);
  });

  it("replaces the run when it finishes and never resurrects it", () => {
    const finished = run({ status: "partiallyCompleted", finishedAt: "2026-11-20T10:00:30Z" });
    const after = applyRunEvent(run(), "r1", { type: "runFinished", run: finished });
    expect(after).toBe(finished);
    expect(applyRunEvent(after, "r1", { type: "runStarted", run: run() })).toBe(finished);
    expect(applyRunEvent(after, "r1", { type: "entryUpdated", runId: "r1", entry: entry("a", "launching") })).toBe(finished);
    expect(applyRunEvent(after, "r1", { type: "windowCloseUpdated", runId: "r1", outcome: outcome("w", "closed") })).toBe(finished);
  });

  it("accepts a start event when the run could not be loaded", () => {
    const started = run();
    expect(applyRunEvent(null, "r1", { type: "runStarted", run: started })).toBe(started);
    expect(applyRunEvent(null, "r1", { type: "entryUpdated", runId: "r1", entry: entry("a", "launching") })).toBeNull();
  });
});

describe("replayRunEvents", () => {
  it("ends with the latest state even when the loaded copy is newer than early events", () => {
    // Loaded after "a" completed; the buffer still holds everything since subscribing.
    const loaded = run({ entries: [entry("a", "completed"), entry("b", "pending")] });
    const events: RestoreEvent[] = [
      { type: "runStarted", run: run() },
      { type: "entryUpdated", runId: "r1", entry: entry("a", "launching") },
      { type: "entryUpdated", runId: "r1", entry: entry("a", "completed") },
      { type: "entryUpdated", runId: "r1", entry: entry("b", "launching") },
    ];
    const replayed = replayRunEvents(loaded, "r1", events);
    expect(replayed?.entries.map((e) => e.status)).toEqual(["completed", "launching"]);
  });

  it("keeps a finished run finished", () => {
    const finished = run({ status: "completed" });
    const replayed = replayRunEvents(finished, "r1", [{ type: "entryUpdated", runId: "r1", entry: entry("a", "launching") }]);
    expect(replayed).toBe(finished);
  });
});

describe("isInterruptedRun", () => {
  const now = Date.parse("2026-11-20T10:05:00Z");
  it("flags runs the backend no longer runs", () => {
    expect(isInterruptedRun(run(), new Set(), true, now)).toBe(true);
  });
  it("trusts active runs, unknown state and young runs", () => {
    expect(isInterruptedRun(run(), new Set(["r1"]), true, now)).toBe(false);
    expect(isInterruptedRun(run(), new Set(), false, now)).toBe(false);
    expect(isInterruptedRun(run({ startedAt: "2026-11-20T10:04:55Z" }), new Set(), true, now)).toBe(false);
    expect(isInterruptedRun(run({ status: "completed" }), new Set(), true, now)).toBe(false);
  });
});

describe("runCounts", () => {
  it("counts outcomes and leaves turned-off apps out", () => {
    const counts = runCounts(
      run({
        entries: [
          entry("a", "completed"),
          entry("b", "partiallyCompleted"),
          entry("c", "failed"),
          entry("d", "skipped", { message: "Skipped because C did not start (required dependency)" }),
          entry("e", "skipped", { message: "Disabled" }),
          entry("f", "launching"),
        ],
      }),
    );
    expect(counts).toEqual({ total: 5, done: 4, restored: 2, completed: 1, partial: 1, failed: 1, skipped: 1, disabled: 1 });
  });
});

describe("retryableEntryIds", () => {
  it("matches the engine's rules", () => {
    const ids = retryableEntryIds(
      run({
        entries: [
          entry("ok", "completed"),
          entry("fail", "failed"),
          entry("part", "partiallyCompleted"),
          entry("dep", "skipped", { message: "Skipped because Docker did not start (required dependency)" }),
          entry("wait", "skipped", { message: "Waiting on Docker" }),
          entry("cancel", "skipped", { message: "Restore cancelled" }),
          entry("off", "skipped", { message: "Disabled" }),
          entry("nomsg", "skipped"),
          entry("busy", "launching"),
        ],
      }),
    );
    expect(ids).toEqual(["fail", "part", "dep", "wait", "cancel"]);
  });
});

describe("runSummary", () => {
  it("celebrates complete restores", () => {
    expect(runSummary(run({ status: "completed", entries: [entry("a", "completed"), entry("b", "completed")] })).title).toBe(
      "Restored all 2 apps",
    );
    expect(runSummary(run({ status: "completed", entries: [entry("a", "completed")] })).title).toBe("Restored 1 app");
  });

  it("says how many apps came back after a partial restore", () => {
    const summary = runSummary(
      run({
        status: "partiallyCompleted",
        entries: [...["a", "b", "c", "d", "e"].map((id) => entry(id, "completed")), entry("f", "failed")],
      }),
    );
    expect(summary.title).toBe("Restored 5 of 6 apps");
    expect(summary.tone).toBe("warning");
    expect(summary.detail).toBe("1 app couldn't be restored. See the details below.");
  });

  it("names partly restored apps instead of counting them as fully restored", () => {
    const summary = runSummary(run({ status: "partiallyCompleted", entries: [entry("a", "partiallyCompleted"), entry("b", "completed")] }));
    expect(summary.title).toBe("Restored 2 apps, 1 only partly");
    expect(summary.detail).toBe("Some windows couldn't be reopened or arranged. See the details below.");
    expect(runSummary(run({ status: "partiallyCompleted", entries: [entry("a", "partiallyCompleted")] })).title).toBe(
      "Partly restored 1 app",
    );
    const mixed = runSummary(
      run({
        status: "partiallyCompleted",
        entries: [entry("a", "completed"), entry("b", "failed"), entry("c", "skipped", { message: "Waiting on B" })],
      }),
    );
    expect(mixed.detail).toBe("1 app couldn't be restored and 1 was skipped. See the details below.");
  });

  it("mentions windows closed by a Replace switch", () => {
    const summary = runSummary(
      run({
        status: "completed",
        entries: [entry("a", "completed")],
        closedWindows: [outcome("w1", "closed"), outcome("w2", "closed"), outcome("w3", "stillOpen")],
      }),
    );
    expect(summary.detail).toBe("Everything is back where you left it. Closed 2 windows. 1 window is still open.");
  });

  it("labels the follow-up action", () => {
    expect(retryLabel(run({ status: "cancelled" }))).toBe("Restore the rest");
    expect(retryLabel(run({ status: "partiallyCompleted", entries: [entry("a", "failed")] }))).toBe("Retry failed");
    expect(retryLabel(run({ status: "partiallyCompleted", entries: [entry("a", "partiallyCompleted")] }))).toBe("Try again");
  });

  it("handles failed, cancelled and empty runs", () => {
    expect(runSummary(run({ status: "failed", entries: [entry("a", "failed")] })).title).toBe("Couldn't restore Development");
    const cancelled = runSummary(run({ status: "cancelled", entries: [entry("a", "completed"), entry("b", "skipped", { message: "Restore cancelled" })] }));
    expect(cancelled.title).toBe("Restore cancelled");
    expect(cancelled.detail).toBe("1 of 2 apps had opened before you cancelled — they stay open.");
    expect(runSummary(run({ status: "completed", entries: [entry("a", "skipped", { message: "Disabled" })] })).title).toBe(
      "Nothing to restore",
    );
  });
});

describe("currentActivity", () => {
  it("names the app being worked on", () => {
    expect(currentActivity(run({ entries: [entry("a", "completed"), entry("b", "launching")] }))).toBe("Opening B…");
    expect(currentActivity(run({ entries: [entry("a", "restoring"), entry("b", "launching"), entry("c", "validating")] }))).toBe(
      "Arranging A and 2 more…",
    );
  });
  it("describes waiting phases", () => {
    expect(currentActivity(run({ closedWindows: [outcome("w", "requested")], entries: [entry("a", "pending")] }))).toBe(
      "Asking apps to close…",
    );
    expect(currentActivity(run())).toBe("Getting ready…");
    expect(currentActivity(run({ entries: [entry("a", "completed")] }))).toBe("Finishing up…");
  });
});

describe("per-app details", () => {
  const workspace = {
    restore: { retryCount: 1 },
    applications: [
      { id: "a", launch: { executable: "/usr/bin/xterm", package: null }, options: { retryCount: null } },
      { id: "b", launch: { executable: null, package: null }, options: { retryCount: 2 } },
    ],
  } as unknown as Workspace;

  it("works out the attempt budget from the workspace", () => {
    expect(maxAttemptsFor(workspace, "a")).toBe(2);
    expect(maxAttemptsFor(workspace, "b")).toBe(3);
    expect(maxAttemptsFor(workspace, "missing")).toBeNull();
    expect(maxAttemptsFor(null, "a")).toBeNull();
  });

  it("mentions attempts only for retries", () => {
    expect(attemptLabel(0, 3)).toBeNull();
    expect(attemptLabel(1, 3)).toBeNull();
    expect(attemptLabel(2, 3)).toBe("Attempt 2 of 3");
    expect(attemptLabel(2, null)).toBe("Attempt 2");
  });

  it("counts windows once arranging starts", () => {
    expect(windowsLabel(entry("a", "launching", { windowsExpected: 3 }))).toBeNull();
    expect(windowsLabel(entry("a", "restoring", { windowsExpected: 3, windowsRestored: 2 }))).toBe("2 of 3 windows");
    expect(windowsLabel(entry("a", "completed", { windowsExpected: 1, windowsRestored: 1 }))).toBe("1 of 1 window");
    expect(windowsLabel(entry("a", "completed", { windowsExpected: 0 }))).toBeNull();
  });

  it("joins the detail line", () => {
    const e = entry("b", "launching", { attempt: 2, message: "Retrying after an error: boom - try again" });
    expect(entryDetail(e, 3)).toBe("Attempt 2 of 3 · Retrying after an error: boom — try again");
    expect(entryDetail(entry("x", "skipped", { message: "Disabled" }), null)).toBe("Turned off in this workspace");
    const partial = entry("x", "partiallyCompleted", { message: "1 of 2 windows reopened", windowsExpected: 2, windowsRestored: 1 });
    expect(entryDetail(partial, null)).toBe("1 of 2 windows reopened");
    const reused = entry("x", "completed", { message: "Already running - arranged windows", windowsExpected: 2, windowsRestored: 2 });
    expect(entryDetail(reused, null)).toBe("Already running — arranged windows · 2 of 2 windows");
    expect(displayMessage(null)).toBeNull();
  });

  it("measures durations", () => {
    expect(entryDurationMs(entry("a", "completed", { startedAt: "2026-11-20T10:00:00Z", finishedAt: "2026-11-20T10:00:02.5Z" }))).toBe(2500);
    expect(entryDurationMs(entry("a", "launching", { startedAt: "2026-11-20T10:00:00Z" }))).toBeNull();
    const now = Date.parse("2026-11-20T10:01:05Z");
    expect(runElapsedMs(run(), now)).toBe(65_000);
    expect(runElapsedMs(run({ finishedAt: "2026-11-20T10:00:14Z" }), now)).toBe(14_000);
  });

  it("formats a stopwatch", () => {
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(7_900)).toBe("0:07");
    expect(formatClock(750_000)).toBe("12:30");
    expect(formatClock(3_723_000)).toBe("1:02:03");
    expect(formatClock(Number.NaN)).toBe("0:00");
  });

  it("builds icon lookups from the workspace", () => {
    expect(entryIconLookup(workspace, { entryId: "a", appId: "xterm" })).toEqual({
      appId: "xterm",
      executablePath: "/usr/bin/xterm",
      package: null,
    });
    expect(entryIconLookup(undefined, { entryId: "a", appId: "xterm" }).executablePath).toBeNull();
  });
});
