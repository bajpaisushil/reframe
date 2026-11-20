import { describe, expect, it, vi } from "vitest";
import {
  createMemoryHistory,
  createRouter,
  matchPattern,
  navSection,
  normalizePath,
  parseRoute,
  pathFromHash,
  routeToPath,
  routesEqual,
  type Route,
} from "./router";

describe("normalizePath", () => {
  it("adds a leading slash, collapses slashes and drops trailing ones", () => {
    expect(normalizePath("")).toBe("/");
    expect(normalizePath("/")).toBe("/");
    expect(normalizePath("explore")).toBe("/explore");
    expect(normalizePath("//workspace//abc/")).toBe("/workspace/abc");
  });

  it("ignores query strings and fragments", () => {
    expect(normalizePath("/settings?tab=startup")).toBe("/settings");
    expect(normalizePath("/activity#top")).toBe("/activity");
  });
});

describe("matchPattern", () => {
  it("matches static segments exactly", () => {
    expect(matchPattern("/explore", "/explore")).toEqual({});
    expect(matchPattern("/explore", "/explorer")).toBeNull();
    expect(matchPattern("/", "/")).toEqual({});
  });

  it("extracts and decodes params", () => {
    expect(matchPattern("/workspace/:id", "/workspace/a%20b")).toEqual({ id: "a b" });
    expect(matchPattern("/restore/:runId", "/restore/123e4567")).toEqual({ runId: "123e4567" });
  });

  it("rejects wrong segment counts, empty params and malformed encoding", () => {
    expect(matchPattern("/workspace/:id", "/workspace")).toBeNull();
    expect(matchPattern("/workspace/:id", "/workspace/a/b")).toBeNull();
    expect(matchPattern("/workspace/:id", "/workspace/%20")).toBeNull();
    expect(matchPattern("/workspace/:id", "/workspace/%E0%A4%A")).toBeNull();
  });
});

describe("parseRoute / routeToPath", () => {
  const routes: Route[] = [
    { name: "home" },
    { name: "explore" },
    { name: "workspaces" },
    { name: "workspace", id: "9b2f2c1e-0000-4000-8000-000000000001" },
    { name: "restore", runId: "run/with spaces" },
    { name: "activity" },
    { name: "settings" },
    { name: "import" },
  ];

  it("round-trips every route", () => {
    for (const route of routes) {
      expect(parseRoute(routeToPath(route))).toEqual(route);
    }
  });

  it("encodes params in paths", () => {
    expect(routeToPath({ name: "restore", runId: "run/with spaces" })).toBe("/restore/run%2Fwith%20spaces");
  });

  it("returns null for unknown paths", () => {
    expect(parseRoute("/nope")).toBeNull();
    expect(parseRoute("/workspace")).toBeNull();
    expect(parseRoute("/workspaces/extra")).toBeNull();
  });

  it("treats the empty hash as home", () => {
    expect(pathFromHash("")).toBe("/");
    expect(pathFromHash("#")).toBe("/");
    expect(pathFromHash("#/settings")).toBe("/settings");
    expect(parseRoute(pathFromHash("#/activity/"))).toEqual({ name: "activity" });
  });

  it("compares routes by path", () => {
    expect(routesEqual({ name: "workspace", id: "a" }, { name: "workspace", id: "a" })).toBe(true);
    expect(routesEqual({ name: "workspace", id: "a" }, { name: "workspace", id: "b" })).toBe(false);
    expect(routesEqual(null, null)).toBe(true);
    expect(routesEqual(null, { name: "home" })).toBe(false);
  });
});

describe("navSection", () => {
  it("maps detail routes to their parent section", () => {
    expect(navSection({ name: "home" })).toBe("home");
    expect(navSection({ name: "workspace", id: "x" })).toBe("workspaces");
    expect(navSection({ name: "restore", runId: "x" })).toBe("workspaces");
    expect(navSection({ name: "import" })).toBe("workspaces");
    expect(navSection({ name: "settings" })).toBe("settings");
    expect(navSection(null)).toBeNull();
  });
});

describe("createRouter", () => {
  it("starts at the adapter's path", () => {
    const router = createRouter(createMemoryHistory("/settings"));
    expect(router.getState()).toEqual({ path: "/settings", route: { name: "settings" } });
    expect(router.canGoBack()).toBe(false);
  });

  it("navigates, notifies subscribers and ignores same-path navigation", () => {
    const history = createMemoryHistory("/");
    const router = createRouter(history);
    const listener = vi.fn();
    router.subscribe(listener);

    router.navigate({ name: "workspace", id: "w1" });
    expect(router.getState().route).toEqual({ name: "workspace", id: "w1" });
    expect(history.entries).toEqual(["/", "/workspace/w1"]);
    expect(listener).toHaveBeenCalledTimes(1);

    router.navigate("/workspace/w1");
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("goes back through in-app history, then falls back to home", () => {
    const history = createMemoryHistory("/explore");
    const router = createRouter(history);
    router.navigate({ name: "workspaces" });
    router.navigate({ name: "workspace", id: "w1" });
    expect(router.canGoBack()).toBe(true);

    router.back();
    expect(router.getState().path).toBe("/workspaces");
    router.back();
    expect(router.getState().path).toBe("/explore");
    expect(router.canGoBack()).toBe(false);

    router.back();
    expect(router.getState().path).toBe("/");
    expect(history.entries[history.index]).toBe("/");
  });

  it("replace does not grow the back stack", () => {
    const router = createRouter(createMemoryHistory("/"));
    router.navigate({ name: "explore" });
    router.navigate({ name: "settings" }, { replace: true });
    router.back();
    expect(router.getState().path).toBe("/");
  });

  it("follows external history changes (back/forward buttons)", () => {
    const history = createMemoryHistory("/");
    const router = createRouter(history);
    const unsubscribe = router.subscribe(() => undefined);
    router.navigate({ name: "activity" });

    history.back();
    for (const listener of history.listeners) listener();
    expect(router.getState().path).toBe("/");
    expect(router.canGoBack()).toBe(false);
    unsubscribe();
    expect(history.listeners.size).toBe(0);
  });

  it("reports unknown paths as a null route", () => {
    const router = createRouter(createMemoryHistory("/"));
    router.navigate("/does-not-exist");
    expect(router.getState()).toEqual({ path: "/does-not-exist", route: null });
  });
});
