// Live map of restore runs: an `activeRuns()` snapshot kept current by `reframe://restore` events.
//
// The store subscribes to backend events only while something is watching it (reference counted)
// and unsubscribes cleanly, including when the asynchronous `listen` resolves after the last
// watcher already left. Events that arrive while the initial snapshot is loading are buffered and
// replayed on top of it, so a run that finishes during startup is never shown as running.

import type { RestoreEvent, RestoreRun } from "@/types";
import { applyRestoreEvent, reconcileActiveRuns, runningRuns, type RunsMap } from "./runs";

export interface ActiveRunsSnapshot {
  /** Every run seen this session (running plus recently finished), by id. */
  runs: RunsMap;
  /** Runs still in progress, oldest first. */
  running: RestoreRun[];
  /** The initial snapshot has loaded (or failed). */
  ready: boolean;
  error: unknown;
}

export interface ActiveRunsDeps {
  /** Subscribe to restore events; resolves with an unsubscribe function. */
  listen: (handler: (event: RestoreEvent) => void) => Promise<() => void>;
  fetchActive: () => Promise<RestoreRun[]>;
  /** Called once per finished run (refresh caches that depend on it). */
  onRunFinished?: (run: RestoreRun) => void;
}

export class ActiveRunsStore {
  private snapshot: ActiveRunsSnapshot = { runs: new Map(), running: [], ready: false, error: null };
  private readonly listeners = new Set<() => void>();
  /** Incremented on every start/stop so callbacks from an old session are ignored. */
  private session = 0;
  private unlisten: (() => void) | null = null;
  private buffer: RestoreEvent[] | null = null;

  constructor(private readonly deps: ActiveRunsDeps) {}

  getSnapshot = (): ActiveRunsSnapshot => this.snapshot;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    if (this.listeners.size === 1) void this.start();
    return () => {
      this.listeners.delete(listener);
      if (this.listeners.size === 0) this.stop();
    };
  };

  /** Re-read the backend's active runs (e.g. after a manual refresh). */
  refresh = async (): Promise<void> => {
    const session = this.session;
    try {
      const active = await this.deps.fetchActive();
      if (session !== this.session) return;
      this.set(reconcileActiveRuns(this.snapshot.runs, active), { error: null });
    } catch (error) {
      if (session === this.session) this.set(this.snapshot.runs, { error });
    }
  };

  private set(runs: RunsMap, patch: Partial<Pick<ActiveRunsSnapshot, "ready" | "error">> = {}): void {
    const next: ActiveRunsSnapshot = {
      runs,
      running: runs === this.snapshot.runs ? this.snapshot.running : runningRuns(runs),
      ready: patch.ready ?? this.snapshot.ready,
      error: "error" in patch ? patch.error : this.snapshot.error,
    };
    if (
      next.runs === this.snapshot.runs &&
      next.ready === this.snapshot.ready &&
      next.error === this.snapshot.error
    ) {
      return;
    }
    this.snapshot = next;
    for (const listener of [...this.listeners]) listener();
  }

  private handle(event: RestoreEvent, session: number): void {
    if (session !== this.session) return;
    if (this.buffer) {
      this.buffer.push(event);
      return;
    }
    this.apply(event);
  }

  private apply(event: RestoreEvent): void {
    const before = this.snapshot.runs;
    const after = applyRestoreEvent(before, event);
    this.set(after);
    if (event.type === "runFinished" && after !== before) this.deps.onRunFinished?.(event.run);
  }

  private async start(): Promise<void> {
    const session = ++this.session;
    this.buffer = [];
    // Without events the snapshot can still be shown, but live progress is missing: report it.
    let listenError: unknown = null;
    try {
      const unlisten = await this.deps.listen((event) => this.handle(event, session));
      if (session !== this.session) {
        unlisten();
        return;
      }
      this.unlisten = unlisten;
    } catch (error) {
      listenError = error;
    }
    if (session !== this.session) return;

    let fetched: RestoreRun[] | null = null;
    let fetchError: unknown = null;
    try {
      fetched = await this.deps.fetchActive();
    } catch (error) {
      fetchError = error;
    }
    if (session !== this.session) return;

    const buffered = this.buffer ?? [];
    this.buffer = null;
    const error = fetchError ?? listenError;
    const runs = fetched ? reconcileActiveRuns(this.snapshot.runs, fetched) : this.snapshot.runs;
    this.set(runs, { ready: true, error });
    for (const event of buffered) this.apply(event);
  }

  private stop(): void {
    this.session += 1;
    this.buffer = null;
    if (this.unlisten) {
      try {
        this.unlisten();
      } catch {
        // The webview may already be tearing down; nothing else to clean up.
      }
      this.unlisten = null;
    }
  }
}
