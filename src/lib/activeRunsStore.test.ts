import { describe, expect, it, vi } from "vitest";
import type { RestoreEvent, RestoreRun } from "@/types";
import { ActiveRunsStore, type ActiveRunsDeps } from "./activeRunsStore";

function run(id: string, overrides: Partial<RestoreRun> = {}): RestoreRun {
  return {
    id,
    workspaceId: `ws-${id}`,
    workspaceName: `Workspace ${id}`,
    mode: "merge",
    status: "running",
    startedAt: "2026-11-20T10:00:00Z",
    finishedAt: null,
    entries: [],
    closedWindows: [],
    messages: [],
    ...overrides,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

/** Fake backend: control when `listen` and `activeRuns` resolve, and emit events. */
function fakeBackend() {
  let handler: ((event: RestoreEvent) => void) | null = null;
  const unlisten = vi.fn(() => {
    handler = null;
  });
  const listenGate = deferred<void>();
  const active = deferred<RestoreRun[]>();
  const deps: ActiveRunsDeps = {
    listen: async (h) => {
      await listenGate.promise;
      handler = h;
      return unlisten;
    },
    fetchActive: () => active.promise,
    onRunFinished: vi.fn(),
  };
  return {
    deps,
    unlisten,
    openListener: () => listenGate.resolve(),
    resolveActive: (runs: RestoreRun[]) => active.resolve(runs),
    rejectActive: (error: unknown) => active.reject(error),
    emit: (event: RestoreEvent) => handler?.(event),
    get listening() {
      return handler !== null;
    },
  };
}

describe("ActiveRunsStore", () => {
  it("loads the snapshot, then follows events", async () => {
    const backend = fakeBackend();
    const store = new ActiveRunsStore(backend.deps);
    store.subscribe(() => undefined);
    backend.openListener();
    await flush();
    backend.resolveActive([run("r1")]);
    await flush();
    expect(store.getSnapshot()).toMatchObject({ ready: true, error: null });
    expect(store.getSnapshot().running.map((r) => r.id)).toEqual(["r1"]);

    backend.emit({ type: "runFinished", run: run("r1", { status: "completed" }) });
    expect(store.getSnapshot().running).toEqual([]);
    expect(store.getSnapshot().runs.get("r1")?.status).toBe("completed");
    expect(backend.deps.onRunFinished).toHaveBeenCalledTimes(1);
  });

  it("replays events that arrive while the snapshot loads, so finished runs stay finished", async () => {
    const backend = fakeBackend();
    const store = new ActiveRunsStore(backend.deps);
    store.subscribe(() => undefined);
    backend.openListener();
    await flush();
    // The run finishes before the (older) snapshot arrives.
    backend.emit({ type: "runFinished", run: run("r1", { status: "failed" }) });
    expect(store.getSnapshot().runs.size).toBe(0);
    backend.resolveActive([run("r1")]);
    await flush();
    expect(store.getSnapshot().runs.get("r1")?.status).toBe("failed");
    expect(store.getSnapshot().running).toEqual([]);
  });

  it("unsubscribes from events when the last watcher leaves, even if listen resolves late", async () => {
    const backend = fakeBackend();
    const store = new ActiveRunsStore(backend.deps);
    const unsubscribe = store.subscribe(() => undefined);
    unsubscribe(); // gone before `listen` resolved
    backend.openListener();
    await flush();
    expect(backend.unlisten).toHaveBeenCalledTimes(1);
    expect(backend.listening).toBe(false);
  });

  it("stops listening when unsubscribed after startup", async () => {
    const backend = fakeBackend();
    const store = new ActiveRunsStore(backend.deps);
    const unsubscribe = store.subscribe(() => undefined);
    backend.openListener();
    backend.resolveActive([]);
    await flush();
    expect(backend.listening).toBe(true);
    unsubscribe();
    expect(backend.unlisten).toHaveBeenCalledTimes(1);
  });

  it("reports a failed snapshot but keeps listening", async () => {
    const backend = fakeBackend();
    const store = new ActiveRunsStore(backend.deps);
    store.subscribe(() => undefined);
    backend.openListener();
    const failure = new Error("backend unavailable");
    backend.rejectActive(failure);
    await flush();
    expect(store.getSnapshot()).toMatchObject({ ready: true, error: failure });
    backend.emit({ type: "runStarted", run: run("r2") });
    expect(store.getSnapshot().running.map((r) => r.id)).toEqual(["r2"]);
  });

  it("notifies subscribers only on changes and keeps snapshots stable", async () => {
    const backend = fakeBackend();
    const store = new ActiveRunsStore(backend.deps);
    const listener = vi.fn();
    store.subscribe(listener);
    backend.openListener();
    backend.resolveActive([]);
    await flush();
    const snapshot = store.getSnapshot();
    const calls = listener.mock.calls.length;
    backend.emit({
      type: "entryUpdated",
      runId: "unknown",
      entry: {
        entryId: "e1",
        displayName: "Firefox",
        appId: "firefox",
        status: "launching",
        message: null,
        attempt: 1,
        windowsExpected: 1,
        windowsRestored: 0,
        startedAt: null,
        finishedAt: null,
      },
    });
    expect(store.getSnapshot()).toBe(snapshot);
    expect(listener).toHaveBeenCalledTimes(calls);
  });
});
