import { describe, expect, it, vi } from "vitest";
import type { IconLookup } from "@/types";
import { IconCache, canFetchIcon, iconCacheKey } from "./appIcons";

const exe = (path: string, appId = "app"): IconLookup => ({ appId, executablePath: path, package: null });
const pkg = (id: string): IconLookup => ({
  appId: "app",
  executablePath: "/usr/bin/ignored",
  package: { kind: "flatpak", id, path: null },
});
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("iconCacheKey / canFetchIcon", () => {
  it("mirrors the backend key: package, then executable, then app id", () => {
    expect(iconCacheKey(pkg("org.gimp.GIMP"))).toBe("pkg:org.gimp.GIMP");
    expect(iconCacheKey(exe("/usr/bin/xterm"))).toBe("exe:/usr/bin/xterm");
    expect(iconCacheKey({ appId: "vscode", executablePath: null, package: null })).toBe("app:vscode");
  });

  it("only fetches when there is something to resolve", () => {
    expect(canFetchIcon(exe("/usr/bin/xterm"))).toBe(true);
    expect(canFetchIcon(pkg("x"))).toBe(true);
    expect(canFetchIcon({ appId: "vscode", executablePath: null, package: null })).toBe(false);
  });
});

describe("IconCache", () => {
  it("de-duplicates concurrent requests and caches the result", async () => {
    const fetcher = vi.fn(async () => "data:image/png;base64,AAA");
    const cache = new IconCache(fetcher);
    expect(cache.peek(exe("/usr/bin/xterm"))).toBeUndefined();
    const [a, b] = await Promise.all([cache.load(exe("/usr/bin/xterm")), cache.load(exe("/usr/bin/xterm", "other"))]);
    expect(a).toBe("data:image/png;base64,AAA");
    expect(b).toBe(a);
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(cache.peek(exe("/usr/bin/xterm"))).toBe(a);
    await cache.load(exe("/usr/bin/xterm"));
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it("caches 'no icon' answers (null and empty strings)", async () => {
    const fetcher = vi.fn(async (lookup: IconLookup) => (lookup.executablePath === "/a" ? null : ""));
    const cache = new IconCache(fetcher);
    expect(await cache.load(exe("/a"))).toBeNull();
    expect(await cache.load(exe("/b"))).toBeNull();
    expect(cache.peek(exe("/a"))).toBeNull();
    await cache.load(exe("/a"));
    expect(fetcher).toHaveBeenCalledTimes(2);
  });

  it("never calls the backend for lookups it cannot resolve", async () => {
    const fetcher = vi.fn(async () => "x");
    const cache = new IconCache(fetcher);
    expect(await cache.load({ appId: "vscode", executablePath: null, package: null })).toBeNull();
    expect(fetcher).not.toHaveBeenCalled();
  });

  it("does not reject on failures and retries after the failure TTL", async () => {
    let now = 0;
    const fetcher = vi
      .fn<(lookup: IconLookup) => Promise<string | null>>()
      .mockRejectedValueOnce(new Error("ipc"))
      .mockResolvedValue("data:ok");
    const cache = new IconCache(fetcher, { failureTtlMs: 1000, now: () => now });
    await expect(cache.load(exe("/x"))).resolves.toBeNull();
    expect(cache.peek(exe("/x"))).toBeNull();
    await cache.load(exe("/x"));
    expect(fetcher).toHaveBeenCalledTimes(1);
    now = 1500;
    expect(cache.peek(exe("/x"))).toBeUndefined();
    await expect(cache.load(exe("/x"))).resolves.toBe("data:ok");
    expect(fetcher).toHaveBeenCalledTimes(2);
  });

  it("handles fetchers that throw synchronously", async () => {
    const cache = new IconCache(() => {
      throw new Error("sync");
    });
    await expect(cache.load(exe("/x"))).resolves.toBeNull();
  });

  it("evicts the least recently used entries beyond the limit", async () => {
    const cache = new IconCache(async (lookup) => `icon:${lookup.executablePath}`, { maxEntries: 2 });
    await cache.load(exe("/1"));
    await cache.load(exe("/2"));
    cache.peek(exe("/1")); // touch: /2 is now the oldest
    await cache.load(exe("/3"));
    await flush();
    expect(cache.size).toBe(2);
    expect(cache.peek(exe("/1"))).toBe("icon:/1");
    expect(cache.peek(exe("/2"))).toBeUndefined();
    expect(cache.peek(exe("/3"))).toBe("icon:/3");
  });
});
