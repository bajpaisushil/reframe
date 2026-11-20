// Labels and edits for one application entry and its saved state. Pure (tested in
// entries.test.ts).

import { pluralize } from "@/lib/format";
import type {
  AppState,
  ApplicationEntry,
  BrowserWindow,
  LaunchConfig,
  LaunchTarget,
  MonitorInfo,
  PackageKind,
  TerminalSession,
  WindowState,
} from "@/types";

const PACKAGE_KIND: Record<PackageKind, string> = {
  macBundle: "App bundle",
  windowsPackage: "Windows app",
  flatpak: "Flatpak",
  snap: "Snap",
  appImage: "AppImage",
};

export function packageKindLabel(kind: PackageKind): string {
  return PACKAGE_KIND[kind] ?? "Package";
}

/** What starts the application: a package id or an executable path (null when unknown). */
export function launchTargetText(launch: Pick<LaunchConfig, "executable" | "package">): string | null {
  if (launch.package) return launch.package.path ?? launch.package.id;
  return launch.executable && launch.executable.trim() !== "" ? launch.executable : null;
}

export function launchTargetLabel(target: LaunchTarget): string {
  switch (target.kind) {
    case "executable":
      return target.path;
    case "package":
      return target.package.path ?? target.package.id;
    case "open":
      return target.target;
  }
}

/** Point an entry at a new program (from a found location or a manually chosen one). */
export function withLaunchTarget(entry: ApplicationEntry, launch: Pick<LaunchConfig, "executable" | "package">): ApplicationEntry {
  return { ...entry, launch: { ...entry.launch, executable: launch.executable, package: launch.package } };
}

/** A launch target found by Reframe, as executable/package fields (null for documents/URLs). */
export function launchFromTarget(target: LaunchTarget): Pick<LaunchConfig, "executable" | "package"> | null {
  switch (target.kind) {
    case "executable":
      return { executable: target.path, package: null };
    case "package":
      return { executable: null, package: target.package };
    case "open":
      return null;
  }
}

const WINDOW_STATE: Record<WindowState, string> = {
  normal: "Windowed",
  maximized: "Maximized",
  minimized: "Minimized",
  fullscreen: "Full screen",
};

export function windowStateLabel(state: WindowState): string {
  return WINDOW_STATE[state] ?? "Windowed";
}

/** Name of the display a window was saved on ("DELL U2720Q"), or null when unknown. */
export function monitorName(monitors: readonly MonitorInfo[], monitorId: string | null): string | null {
  if (!monitorId) return null;
  const monitor = monitors.find((m) => m.id === monitorId);
  return monitor ? monitor.name || monitor.id : null;
}

export function windowCountLabel(entry: Pick<ApplicationEntry, "windows">): string {
  return pluralize(entry.windows.length, "window");
}

/** Heading and item count for an entry's saved content, or null when it only launches. */
export function stateSummary(state: AppState): { label: string; count: number | null } | null {
  switch (state.kind) {
    case "projects":
      return { label: "Projects", count: state.projects.length };
    case "browser":
      return { label: "Tabs", count: state.windows.reduce((sum, w) => sum + w.tabs.length, 0) };
    case "terminal":
      return { label: "Sessions", count: state.sessions.length };
    case "folders":
      return { label: "Folders", count: state.folders.length };
    case "documents":
      return { label: "Documents", count: state.documents.length };
    case "media":
      return { label: "Playlist", count: null };
    case "custom":
      return { label: "Saved data", count: null };
    case "none":
    default:
      return null;
  }
}

function names(paths: readonly string[], max = 2): string {
  const leaves = paths.map(pathLeaf);
  const shown = leaves.slice(0, max).join(", ");
  return leaves.length > max ? `${shown} +${leaves.length - max}` : shown;
}

/**
 * A one-line description of what an entry reopens, from its (possibly unsaved) state:
 * "reframe, website +1", "12 tabs in 2 windows", "2 sessions". Empty when it only launches.
 */
export function stateLine(state: AppState): string {
  switch (state.kind) {
    case "projects":
      return names(state.projects.map((p) => p.path));
    case "browser": {
      // Tabs only: "windows" in the row means saved window positions.
      const tabs = state.windows.reduce((sum, w) => sum + w.tabs.length, 0);
      return tabs === 0 ? "" : pluralize(tabs, "tab");
    }
    case "terminal":
      return state.sessions.length === 0 ? "" : pluralize(state.sessions.length, "session");
    case "folders":
      return names(state.folders.map((f) => f.path));
    case "documents":
      return names(state.documents.map((d) => d.path));
    case "media":
      return state.label?.trim() || (state.uri?.trim() ? "Playlist" : "");
    default:
      return "";
  }
}

/** Last path segment for compact labels ("/home/me/code/reframe" → "reframe"). */
export function pathLeaf(path: string): string {
  const parts = path.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** 1 → "1st", 2 → "2nd", 11 → "11th". */
export function ordinal(n: number): string {
  const tens = n % 100;
  const suffix =
    tens >= 11 && tens <= 13 ? "th" : n % 10 === 1 ? "st" : n % 10 === 2 ? "nd" : n % 10 === 3 ? "rd" : "th";
  return `${n}${suffix}`;
}

export function removeAt<T>(list: readonly T[], index: number): T[] {
  return list.filter((_, i) => i !== index);
}

export function replaceAt<T>(list: readonly T[], index: number, item: T): T[] {
  return list.map((existing, i) => (i === index ? item : existing));
}

// ---- Browser ------------------------------------------------------------------------------------

/** Remove a tab; a window left without tabs is removed, and the selected tab stays selected. */
export function removeBrowserTab(windows: readonly BrowserWindow[], windowIndex: number, tabIndex: number): BrowserWindow[] {
  const target = windows[windowIndex];
  if (!target || tabIndex < 0 || tabIndex >= target.tabs.length) return windows.slice();
  const tabs = removeAt(target.tabs, tabIndex);
  if (tabs.length === 0) return removeAt(windows, windowIndex);
  let activeTab = target.activeTab;
  if (activeTab !== null) {
    if (activeTab === tabIndex) activeTab = Math.min(tabIndex, tabs.length - 1);
    else if (activeTab > tabIndex) activeTab -= 1;
  }
  return replaceAt(windows, windowIndex, { ...target, tabs, activeTab });
}

/** Add a tab at the end of a window (a first window is created when there is none). */
export function addBrowserTab(
  windows: readonly BrowserWindow[],
  windowIndex: number,
  tab: { url: string; title: string },
): BrowserWindow[] {
  const newTab = { url: tab.url, title: tab.title, pinned: false };
  const target = windows[windowIndex];
  if (!target) return [...windows, { tabs: [newTab], activeTab: 0, windowTitle: null }];
  return replaceAt(windows, windowIndex, { ...target, tabs: [...target.tabs, newTab] });
}

// ---- Terminal -----------------------------------------------------------------------------------

export function emptyTerminalSession(): TerminalSession {
  return { workingDirectory: null, profile: null, startupCommand: null, startupCommandApproved: false, windowTitle: null };
}

/**
 * Update a terminal session. Changing the startup command always withdraws its approval: a
 * command runs only after the user has approved exactly that text.
 */
export function updateTerminalSession(session: TerminalSession, patch: Partial<TerminalSession>): TerminalSession {
  const next = { ...session, ...patch };
  const commandChanged = patch.startupCommand !== undefined && patch.startupCommand !== session.startupCommand;
  if (commandChanged && patch.startupCommandApproved === undefined) next.startupCommandApproved = false;
  if (next.startupCommand === null || next.startupCommand.trim() === "") next.startupCommandApproved = false;
  return next;
}

/** Commands that are configured but will not run because nobody approved them. */
export function unapprovedCommands(state: AppState): number {
  if (state.kind !== "terminal") return 0;
  return state.sessions.filter((s) => s.startupCommand && s.startupCommand.trim() !== "" && !s.startupCommandApproved).length;
}
