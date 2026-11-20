// A tiny, typed hash router (no dependency). Routes are a discriminated union, so pages get
// typed params and links cannot point at routes that do not exist.
//
//   navigate({ name: "workspace", id })      push a route
//   navigate({ name: "home" }, { replace: true })
//   back()                                   previous in-app route, or Home when there is none
//   const { route } = useRoute()             current route (null for an unknown path)
//   <Link to={{ name: "activity" }}>View all</Link>
//
// Hash URLs (`#/workspace/<id>`) survive webview reloads and work with Tauri's custom protocol.

import { useSyncExternalStore, type AnchorHTMLAttributes, type MouseEvent, type ReactNode } from "react";

export type Route =
  | { name: "home" }
  | { name: "explore" }
  | { name: "workspaces" }
  | { name: "workspace"; id: string }
  | { name: "restore"; runId: string }
  | { name: "activity" }
  | { name: "settings" }
  | { name: "import" };

export type RouteName = Route["name"];

/** Sidebar sections; detail routes highlight their parent section. */
export type NavSection = "home" | "explore" | "workspaces" | "activity" | "settings";

const PATTERNS: Record<RouteName, string> = {
  home: "/",
  explore: "/explore",
  workspaces: "/workspaces",
  workspace: "/workspace/:id",
  restore: "/restore/:runId",
  activity: "/activity",
  settings: "/settings",
  import: "/import",
};

/** Order matters only for readability; patterns never overlap. */
const ROUTE_NAMES = Object.keys(PATTERNS) as RouteName[];

function safeDecode(segment: string): string | null {
  try {
    return decodeURIComponent(segment);
  } catch {
    return null;
  }
}

/** Strip query/fragment, collapse duplicate slashes and drop a trailing slash. */
export function normalizePath(path: string): string {
  const withoutQuery = path.split(/[?#]/, 1)[0] ?? "";
  const collapsed = `/${withoutQuery}`.replace(/\/{2,}/g, "/");
  return collapsed.length > 1 ? collapsed.replace(/\/$/, "") : "/";
}

/**
 * Match `/workspace/:id`-style patterns. Returns decoded params, or `null` when the path does not
 * match (including malformed percent-encoding and empty params).
 */
export function matchPattern(pattern: string, path: string): Record<string, string> | null {
  const patternParts = normalizePath(pattern).split("/").filter(Boolean);
  const pathParts = normalizePath(path).split("/").filter(Boolean);
  if (patternParts.length !== pathParts.length) return null;
  const params: Record<string, string> = {};
  for (let i = 0; i < patternParts.length; i++) {
    const expected = patternParts[i];
    const actual = pathParts[i];
    if (expected.startsWith(":")) {
      const value = safeDecode(actual);
      if (value === null || value.trim() === "") return null;
      params[expected.slice(1)] = value;
    } else if (expected !== actual) {
      return null;
    }
  }
  return params;
}

/** Parse a path ("/workspace/abc") into a typed route; `null` when nothing matches. */
export function parseRoute(path: string): Route | null {
  for (const name of ROUTE_NAMES) {
    const params = matchPattern(PATTERNS[name], path);
    if (!params) continue;
    switch (name) {
      case "workspace":
        return { name, id: params.id };
      case "restore":
        return { name, runId: params.runId };
      default:
        return { name } as Route;
    }
  }
  return null;
}

/** Path for a route (params are percent-encoded). */
export function routeToPath(route: Route): string {
  switch (route.name) {
    case "workspace":
      return `/workspace/${encodeURIComponent(route.id)}`;
    case "restore":
      return `/restore/${encodeURIComponent(route.runId)}`;
    default:
      return PATTERNS[route.name];
  }
}

/** "#/explore" -> "/explore"; an empty hash is Home. */
export function pathFromHash(hash: string): string {
  const raw = hash.startsWith("#") ? hash.slice(1) : hash;
  return normalizePath(raw || "/");
}

export function routesEqual(a: Route | null, b: Route | null): boolean {
  if (!a || !b) return a === b;
  return routeToPath(a) === routeToPath(b);
}

/** Which sidebar item a route belongs to. */
export function navSection(route: Route | null): NavSection | null {
  if (!route) return null;
  switch (route.name) {
    case "home":
      return "home";
    case "explore":
      return "explore";
    case "workspaces":
    case "workspace":
    case "restore":
    case "import":
      return "workspaces";
    case "activity":
      return "activity";
    case "settings":
      return "settings";
  }
}

// ---------------------------------------------------------------------------------------------
// History store
// ---------------------------------------------------------------------------------------------

/** Where the router reads and writes the current path. */
export interface HistoryAdapter {
  read(): string;
  push(path: string): void;
  replace(path: string): void;
  /** Go back one entry; the adapter reports the new path through `listen`. */
  back(): void;
  /** Called when the path changes outside `push`/`replace` (back/forward buttons). */
  listen(onChange: () => void): () => void;
}

export interface RouterState {
  path: string;
  route: Route | null;
}

export interface NavigateOptions {
  replace?: boolean;
}

export interface Router {
  getState(): RouterState;
  subscribe(listener: () => void): () => void;
  navigate(to: Route | string, options?: NavigateOptions): void;
  back(fallback?: Route): void;
  canGoBack(): boolean;
}

/** Create a router over a history adapter (browser hash history in the app, memory in tests). */
export function createRouter(adapter: HistoryAdapter): Router {
  const listeners = new Set<() => void>();
  const initialPath = normalizePath(adapter.read());
  let state: RouterState = { path: initialPath, route: parseRoute(initialPath) };
  // In-app history for this session, so `back()` never leaves the app.
  const stack: string[] = [initialPath];
  let detach: (() => void) | null = null;

  const emit = () => {
    for (const listener of [...listeners]) listener();
  };

  const setPath = (path: string) => {
    if (path === state.path) return;
    state = { path, route: parseRoute(path) };
    emit();
  };

  const onExternalChange = () => {
    const path = normalizePath(adapter.read());
    if (path === state.path) return;
    if (stack.length > 1 && stack[stack.length - 2] === path) stack.pop();
    else stack.push(path);
    setPath(path);
  };

  return {
    getState: () => state,
    subscribe(listener) {
      listeners.add(listener);
      if (!detach) detach = adapter.listen(onExternalChange);
      return () => {
        listeners.delete(listener);
        if (listeners.size === 0 && detach) {
          detach();
          detach = null;
        }
      };
    },
    navigate(to, options = {}) {
      const path = normalizePath(typeof to === "string" ? to : routeToPath(to));
      if (path === state.path) return;
      if (options.replace) {
        stack[stack.length - 1] = path;
        adapter.replace(path);
      } else {
        stack.push(path);
        adapter.push(path);
      }
      setPath(path);
    },
    back(fallback = { name: "home" }) {
      if (stack.length > 1) {
        stack.pop();
        const target = stack[stack.length - 1];
        adapter.back();
        // Adapters that report asynchronously (browser history) will confirm via `listen`;
        // updating now keeps the UI responsive and is a no-op when they do.
        setPath(target);
        return;
      }
      const path = routeToPath(fallback);
      if (path === state.path) return;
      stack[stack.length - 1] = path;
      adapter.replace(path);
      setPath(path);
    },
    canGoBack: () => stack.length > 1,
  };
}

/** In-memory history (tests, non-browser environments). */
export interface MemoryHistory extends HistoryAdapter {
  entries: string[];
  index: number;
  /** Subscribers registered through `listen` (tests fire them to simulate back/forward). */
  listeners: Set<() => void>;
}

export function createMemoryHistory(initialPath = "/"): MemoryHistory {
  const history: MemoryHistory = {
    entries: [normalizePath(initialPath)],
    index: 0,
    listeners: new Set<() => void>(),
    read() {
      return history.entries[history.index];
    },
    push(path: string) {
      history.entries = history.entries.slice(0, history.index + 1);
      history.entries.push(path);
      history.index = history.entries.length - 1;
    },
    replace(path: string) {
      history.entries[history.index] = path;
    },
    back() {
      if (history.index > 0) history.index -= 1;
    },
    listen(onChange: () => void) {
      history.listeners.add(onChange);
      return () => history.listeners.delete(onChange);
    },
  };
  return history;
}

/** Browser history using the URL hash. */
function createHashHistory(): HistoryAdapter {
  const url = (path: string) => `${window.location.pathname}${window.location.search}#${path}`;
  return {
    read: () => pathFromHash(window.location.hash),
    push: (path) => window.history.pushState(null, "", url(path)),
    replace: (path) => window.history.replaceState(null, "", url(path)),
    back: () => window.history.back(),
    listen(onChange) {
      window.addEventListener("popstate", onChange);
      window.addEventListener("hashchange", onChange);
      return () => {
        window.removeEventListener("popstate", onChange);
        window.removeEventListener("hashchange", onChange);
      };
    },
  };
}

let appRouter: Router | null = null;

/** The app-wide router (created on first use, hash history in the browser). */
export function getRouter(): Router {
  if (!appRouter) {
    appRouter = createRouter(typeof window === "undefined" ? createMemoryHistory() : createHashHistory());
  }
  return appRouter;
}

export function navigate(to: Route | string, options?: NavigateOptions): void {
  getRouter().navigate(to, options);
}

export function back(fallback?: Route): void {
  getRouter().back(fallback);
}

/** Current route; re-renders on navigation. */
export function useRoute(): RouterState {
  const router = getRouter();
  return useSyncExternalStore(router.subscribe, router.getState, router.getState);
}

type LinkProps = Omit<AnchorHTMLAttributes<HTMLAnchorElement>, "href"> & {
  to: Route;
  replace?: boolean;
  children: ReactNode;
};

/** An in-app link: a real anchor (keyboard and screen-reader friendly) that navigates in place. */
export function Link({ to, replace, onClick, children, ...rest }: LinkProps) {
  const handleClick = (event: MouseEvent<HTMLAnchorElement>) => {
    onClick?.(event);
    if (event.defaultPrevented || event.button !== 0) return;
    event.preventDefault();
    navigate(to, { replace });
  };
  return (
    <a href={`#${routeToPath(to)}`} onClick={handleClick} {...rest}>
      {children}
    </a>
  );
}
