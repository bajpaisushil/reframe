import { describe, expect, it, vi } from "vitest";
import { QueryClient, keyMatches } from "./queryClient";

/** A promise you resolve or reject by hand. */
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

describe("keyMatches", () => {
  it("matches exact keys and colon-separated children", () => {
    expect(keyMatches("activity", "activity")).toBe(true);
    expect(keyMatches("activity:6", "activity")).toBe(true);
    expect(keyMatches("activityFeed", "activity")).toBe(false);
    expect(keyMatches("workspace:1", "workspaces")).toBe(false);
  });
});

describe("QueryClient", () => {
  it("loads data and reports status transitions", async () => {
    const client = new QueryClient();
    const listener = vi.fn();
    client.subscribe("k", listener);
    expect(client.getState("k").status).toBe("idle");

    const request = client.fetch("k", async () => 42);
    expect(client.getState("k")).toMatchObject({ status: "loading", isFetching: true });
    await expect(request).resolves.toBe(42);
    expect(client.getState("k")).toMatchObject({ status: "success", data: 42, isFetching: false, error: null });
    expect(listener).toHaveBeenCalled();
  });

  it("shares one in-flight request between callers", async () => {
    const client = new QueryClient();
    const fetcher = vi.fn(async () => "value");
    const [a, b] = await Promise.all([client.fetch("k", fetcher), client.fetch("k", fetcher)]);
    expect(a).toBe("value");
    expect(b).toBe("value");
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it("reuses fresh data and refetches stale data", async () => {
    let now = 1_000;
    const client = new QueryClient(() => now);
    const fetcher = vi.fn(async () => now);
    await client.fetch("k", fetcher, { staleTime: 500 });
    await client.fetch("k", fetcher, { staleTime: 500 });
    expect(fetcher).toHaveBeenCalledTimes(1);
    now += 600;
    await client.fetch("k", fetcher, { staleTime: 500 });
    expect(fetcher).toHaveBeenCalledTimes(2);
  });

  it("ignores a slow response once a newer request was started", async () => {
    const client = new QueryClient();
    const slow = deferred<string>();
    const fast = deferred<string>();
    const first = client.fetch("k", () => slow.promise);
    const second = client.fetch("k", () => fast.promise, { force: true });

    fast.resolve("new");
    await expect(second).resolves.toBe("new");
    slow.resolve("old");
    await expect(first).resolves.toBeUndefined();
    expect(client.getState("k").data).toBe("new");
  });

  it("stores errors without rejecting and keeps previous data", async () => {
    const client = new QueryClient();
    await client.fetch("k", async () => "good");
    const failure = new Error("boom");
    await expect(
      client.fetch(
        "k",
        async () => {
          throw failure;
        },
        { force: true },
      ),
    ).resolves.toBeUndefined();
    expect(client.getState("k")).toMatchObject({ status: "error", error: failure, data: "good", isFetching: false });
  });

  it("captures synchronous throws from the fetcher", async () => {
    const client = new QueryClient();
    const result = await client.fetch("k", () => {
      throw new Error("sync");
    });
    expect(result).toBeUndefined();
    expect(client.getState("k").status).toBe("error");
  });

  it("setData supersedes in-flight requests", async () => {
    const client = new QueryClient();
    const pending = deferred<number>();
    const request = client.fetch("k", () => pending.promise);
    client.setData<number>("k", (previous) => (previous ?? 0) + 5);
    pending.resolve(1);
    await expect(request).resolves.toBeUndefined();
    expect(client.getState("k").data).toBe(5);
  });

  it("invalidate refetches watched keys by prefix and marks the rest stale", async () => {
    const client = new QueryClient();
    const watched = vi.fn(async () => "watched");
    const unwatched = vi.fn(async () => "unwatched");
    const other = vi.fn(async () => "other");
    await client.fetch("activity:6", watched);
    await client.fetch("activity:50", unwatched);
    await client.fetch("workspaces", other);
    client.subscribe("activity:6", () => undefined);

    client.invalidate("activity");
    await flush();
    expect(watched).toHaveBeenCalledTimes(2);
    expect(unwatched).toHaveBeenCalledTimes(1);
    expect(other).toHaveBeenCalledTimes(1);
    expect(client.isStale("activity:50")).toBe(true);
    expect(client.isStale("workspaces", 60_000)).toBe(false);
  });

  it("refetches stale, watched queries on focus only when opted in", async () => {
    let now = 0;
    const client = new QueryClient(() => now);
    const optedIn = vi.fn(async () => 1);
    const optedOut = vi.fn(async () => 2);
    client.configure("a", optedIn, { staleTime: 100, refetchOnFocus: true });
    client.configure("b", optedOut, { staleTime: 100, refetchOnFocus: false });
    await client.fetch("a", optedIn);
    await client.fetch("b", optedOut);
    client.subscribe("a", () => undefined);
    client.subscribe("b", () => undefined);

    client.refetchOnFocus();
    await flush();
    expect(optedIn).toHaveBeenCalledTimes(1); // still fresh

    now = 500;
    client.refetchOnFocus();
    await flush();
    expect(optedIn).toHaveBeenCalledTimes(2);
    expect(optedOut).toHaveBeenCalledTimes(1);
  });

  it("returns a stable state object until something changes", async () => {
    const client = new QueryClient();
    await client.fetch("k", async () => "x");
    const first = client.getState("k");
    expect(client.getState("k")).toBe(first);
    client.setData("k", "y");
    expect(client.getState("k")).not.toBe(first);
  });
});
