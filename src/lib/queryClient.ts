// A small query cache shared by the data hooks (see src/hooks/useQuery.ts).
//
// - One entry per string key ("workspaces", "activity:6", "workspace:<id>").
// - Concurrent fetches of the same key share one request.
// - Stale-response safe: every fetch gets a request id and only the newest one may write.
// - `invalidate("activity")` also matches "activity:6": refetches what is on screen, marks the
//   rest stale for the next mount.
// - Fetches never reject; errors are stored in the entry (no unhandled promise rejections).

export type QueryStatus = "idle" | "loading" | "success" | "error";

export interface QueryState<T> {
  data: T | undefined;
  error: unknown;
  status: QueryStatus;
  /** A request is in flight (initial load or background refresh). */
  isFetching: boolean;
  /** Epoch ms of the last successful fetch or manual write (0 = never). */
  updatedAt: number;
}

export interface FetchOptions {
  /** Ignore cached data and any in-flight request. */
  force?: boolean;
  /** Cached data younger than this is returned without a request. */
  staleTime?: number;
}

interface Entry {
  state: QueryState<unknown>;
  listeners: Set<() => void>;
  fetcher: (() => Promise<unknown>) | null;
  inflight: Promise<unknown> | null;
  requestId: number;
  /** When the data became fresh; null = never loaded, or invalidated since. */
  freshAt: number | null;
  staleTime: number;
  refetchOnFocus: boolean;
}

const IDLE: QueryState<never> = Object.freeze({
  data: undefined,
  error: null,
  status: "idle",
  isFetching: false,
  updatedAt: 0,
}) as QueryState<never>;

/** True when `key` is `prefix` or starts with `prefix:`. */
export function keyMatches(key: string, prefix: string): boolean {
  return key === prefix || key.startsWith(`${prefix}:`);
}

export class QueryClient {
  private readonly entries = new Map<string, Entry>();

  constructor(private readonly now: () => number = Date.now) {}

  private entry(key: string): Entry {
    let entry = this.entries.get(key);
    if (!entry) {
      entry = {
        state: IDLE,
        listeners: new Set(),
        fetcher: null,
        inflight: null,
        requestId: 0,
        freshAt: null,
        staleTime: 0,
        refetchOnFocus: false,
      };
      this.entries.set(key, entry);
    }
    return entry;
  }

  private update(key: string, patch: Partial<QueryState<unknown>>): void {
    const entry = this.entry(key);
    entry.state = { ...entry.state, ...patch };
    for (const listener of [...entry.listeners]) listener();
  }

  /** Current state; the object identity changes only when the state changes. */
  getState<T>(key: string): QueryState<T> {
    return (this.entries.get(key)?.state ?? IDLE) as QueryState<T>;
  }

  subscribe(key: string, listener: () => void): () => void {
    const entry = this.entry(key);
    entry.listeners.add(listener);
    return () => {
      entry.listeners.delete(listener);
    };
  }

  /** Remember how to (re)fetch a key and its freshness options (used by invalidate/focus). */
  configure(key: string, fetcher: () => Promise<unknown>, options: { staleTime?: number; refetchOnFocus?: boolean } = {}): void {
    const entry = this.entry(key);
    entry.fetcher = fetcher;
    entry.staleTime = options.staleTime ?? entry.staleTime;
    entry.refetchOnFocus = options.refetchOnFocus ?? entry.refetchOnFocus;
  }

  isStale(key: string, staleTime?: number): boolean {
    const entry = this.entries.get(key);
    if (!entry || entry.freshAt === null) return true;
    return this.now() - entry.freshAt >= (staleTime ?? entry.staleTime);
  }

  /**
   * Fetch (or reuse) data for `key`. Resolves with the data, or `undefined` when the request
   * failed or was superseded by a newer one. Never rejects.
   */
  fetch<T>(key: string, fetcher: () => Promise<T>, options: FetchOptions = {}): Promise<T | undefined> {
    const entry = this.entry(key);
    entry.fetcher = fetcher;
    if (!options.force) {
      if (entry.inflight) return entry.inflight as Promise<T | undefined>;
      if (entry.state.status === "success" && !this.isStale(key, options.staleTime ?? entry.staleTime)) {
        return Promise.resolve(entry.state.data as T);
      }
    }
    const requestId = ++entry.requestId;
    this.update(key, {
      isFetching: true,
      status: entry.state.data === undefined && entry.state.status !== "success" ? "loading" : entry.state.status,
    });

    let request: Promise<T>;
    try {
      request = Promise.resolve(fetcher());
    } catch (error) {
      request = Promise.reject(error);
    }

    const settled = request.then(
      (data) => {
        if (entry.requestId !== requestId) return undefined;
        entry.inflight = null;
        entry.freshAt = this.now();
        this.update(key, { data, error: null, status: "success", isFetching: false, updatedAt: entry.freshAt });
        return data;
      },
      (error: unknown) => {
        if (entry.requestId !== requestId) return undefined;
        entry.inflight = null;
        this.update(key, { error, status: "error", isFetching: false });
        return undefined;
      },
    );
    entry.inflight = settled;
    return settled;
  }

  /** Write data directly (optimistic updates, mutation results). */
  setData<T>(key: string, updater: T | ((previous: T | undefined) => T)): void {
    const entry = this.entry(key);
    const previous = entry.state.data as T | undefined;
    const data = typeof updater === "function" ? (updater as (p: T | undefined) => T)(previous) : updater;
    // A manual write supersedes whatever is in flight.
    entry.requestId += 1;
    entry.inflight = null;
    entry.freshAt = this.now();
    this.update(key, { data, error: null, status: "success", isFetching: false, updatedAt: entry.freshAt });
  }

  /**
   * Mark matching keys stale. Keys someone is watching are refetched right away (their old data
   * stays visible meanwhile); unwatched keys refetch on their next use.
   */
  invalidate(...prefixes: string[]): void {
    for (const [key, entry] of this.entries) {
      if (!prefixes.some((prefix) => keyMatches(key, prefix))) continue;
      entry.freshAt = null;
      if (entry.listeners.size > 0 && entry.fetcher) {
        void this.fetch(key, entry.fetcher, { force: true });
      }
    }
  }

  /** Refetch watched, stale queries that opted into focus refetching (window regained focus). */
  refetchOnFocus(): void {
    for (const [key, entry] of this.entries) {
      if (!entry.refetchOnFocus || entry.listeners.size === 0 || !entry.fetcher) continue;
      if (entry.inflight || !this.isStale(key)) continue;
      void this.fetch(key, entry.fetcher, { force: true });
    }
  }

  /** Drop everything (tests). */
  clear(): void {
    this.entries.clear();
  }
}

/** The app-wide cache. */
export const queryClient = new QueryClient();

/** Query keys used across features, so invalidation stays consistent. */
export const queryKeys = {
  workspaces: "workspaces",
  workspace: (id: string) => `workspace:${id}`,
  activeWorkspace: "activeWorkspace",
  activity: (limit: number) => `activity:${limit}`,
  activityAll: "activity",
  settings: "settings",
  platformInfo: "platformInfo",
  appVersion: "appVersion",
  desktopSnapshot: "desktopSnapshot",
  restoreHistory: "restoreHistory",
  restoreRun: (runId: string) => `restoreRun:${runId}`,
} as const;
