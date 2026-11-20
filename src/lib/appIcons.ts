// In-memory cache for application icons fetched through `api.appIcon`.
//
// - Keys mirror the backend's icon cache key (package id, else executable path, else app id),
//   so two lookups for the same app share one entry.
// - Concurrent requests for the same key share one IPC call.
// - "No icon" answers are cached for the session; failed calls are retried after a short delay.
// - The cache is bounded (least recently used entries are evicted).

import type { IconLookup } from "@/types";

/** Same key scheme as `IconLookup::cache_key` in reframe-core. */
export function iconCacheKey(lookup: IconLookup): string {
  if (lookup.package) return `pkg:${lookup.package.id}`;
  if (lookup.executablePath) return `exe:${lookup.executablePath}`;
  return `app:${lookup.appId}`;
}

/** The backend can only resolve icons from an executable or a package. */
export function canFetchIcon(lookup: IconLookup): boolean {
  return Boolean(lookup.package || lookup.executablePath);
}

export interface IconCacheOptions {
  maxEntries?: number;
  /** How long a failed request is remembered before trying again. */
  failureTtlMs?: number;
  now?: () => number;
}

export class IconCache {
  private readonly icons = new Map<string, string | null>();
  private readonly inflight = new Map<string, Promise<string | null>>();
  private readonly failures = new Map<string, number>();
  private readonly maxEntries: number;
  private readonly failureTtlMs: number;
  private readonly now: () => number;

  constructor(
    private readonly fetcher: (lookup: IconLookup) => Promise<string | null>,
    options: IconCacheOptions = {},
  ) {
    this.maxEntries = options.maxEntries ?? 400;
    this.failureTtlMs = options.failureTtlMs ?? 60_000;
    this.now = options.now ?? Date.now;
  }

  /**
   * Synchronous lookup: a data URL, `null` when the app has no icon (or it cannot be fetched),
   * or `undefined` when unknown yet.
   */
  peek(lookup: IconLookup): string | null | undefined {
    if (!canFetchIcon(lookup)) return null;
    const key = iconCacheKey(lookup);
    if (this.icons.has(key)) {
      const value = this.icons.get(key) ?? null;
      // Refresh recency.
      this.icons.delete(key);
      this.icons.set(key, value);
      return value;
    }
    const failedAt = this.failures.get(key);
    if (failedAt !== undefined && this.now() - failedAt < this.failureTtlMs) return null;
    return undefined;
  }

  /** Resolve the icon (cached, shared or fetched). Never rejects. */
  load(lookup: IconLookup): Promise<string | null> {
    const cached = this.peek(lookup);
    if (cached !== undefined) return Promise.resolve(cached);
    const key = iconCacheKey(lookup);
    const pending = this.inflight.get(key);
    if (pending) return pending;

    let request: Promise<string | null>;
    try {
      request = Promise.resolve(this.fetcher(lookup));
    } catch (error) {
      request = Promise.reject(error);
    }
    const settled = request.then(
      (icon) => {
        const value = typeof icon === "string" && icon.length > 0 ? icon : null;
        this.failures.delete(key);
        this.store(key, value);
        return value;
      },
      () => {
        this.failures.set(key, this.now());
        return null;
      },
    );
    const tracked = settled.finally(() => this.inflight.delete(key));
    this.inflight.set(key, tracked);
    return tracked;
  }

  private store(key: string, value: string | null): void {
    this.icons.delete(key);
    this.icons.set(key, value);
    while (this.icons.size > this.maxEntries) {
      const oldest = this.icons.keys().next().value;
      if (oldest === undefined) break;
      this.icons.delete(oldest);
    }
  }

  get size(): number {
    return this.icons.size;
  }
}
