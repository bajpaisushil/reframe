// Pure helpers for the Settings page: mapping UI choices onto AppSettings, validating inputs the
// same way the backend does, and describing the platform honestly. Tested in settingsModel.test.ts.

import type {
  AppSettings,
  DisplayServer,
  OsKind,
  PermissionKind,
  PlatformCapabilities,
  PlatformInfo,
  StartupAction,
  SupportLevel,
  WorkspaceSummary,
} from "@/types";

/** Limits enforced by `AppSettings::validate` in the engine. */
export const CLOSE_WAIT_MIN = 1;
export const CLOSE_WAIT_MAX = 120;
export const MAX_EXCLUDED_APPS = 500;

type WorkspaceRef = Pick<WorkspaceSummary, "id" | "isActive">;

/**
 * Apply a startup choice. Choosing "Restore a workspace" picks one right away (the active
 * workspace, else the first) so the settings never become invalid; the workspace is remembered
 * when switching to another choice and back.
 */
export function withStartupAction(settings: AppSettings, action: StartupAction, workspaces: readonly WorkspaceRef[]): AppSettings {
  let workspaceId = settings.startup.workspaceId;
  const known = workspaceId !== null && workspaces.some((w) => w.id === workspaceId);
  if (action === "restoreWorkspace" && !known) {
    workspaceId = (workspaces.find((w) => w.isActive) ?? workspaces[0])?.id ?? null;
  }
  return { ...settings, startup: { ...settings.startup, action, workspaceId } };
}

export function withStartupWorkspace(settings: AppSettings, workspaceId: string): AppSettings {
  return { ...settings, startup: { ...settings.startup, workspaceId } };
}

/** The chosen startup workspace is missing (deleted, or never chosen). */
export function startupWorkspaceMissing(settings: AppSettings, workspaces: readonly WorkspaceRef[]): boolean {
  const id = settings.startup.workspaceId;
  return settings.startup.action === "restoreWorkspace" && (id === null || !workspaces.some((w) => w.id === id));
}

/** Startup restore only runs when Reframe is opened at login. */
export function startupRestoreNeedsLogin(settings: AppSettings): boolean {
  return settings.startup.action !== "doNothing" && !settings.startup.launchAtLogin;
}

export type Parsed<T> = { ok: true; value: T } | { ok: false; error: string };

/** Seconds to wait for windows to close (whole seconds, 1-120). */
export function parseCloseWait(input: string): Parsed<number> {
  const text = input.trim();
  if (text === "") return { ok: false, error: "Enter a number of seconds." };
  if (!/^\d+$/.test(text)) return { ok: false, error: `Use whole seconds, from ${CLOSE_WAIT_MIN} to ${CLOSE_WAIT_MAX}.` };
  const value = Number(text);
  if (value < CLOSE_WAIT_MIN || value > CLOSE_WAIT_MAX) {
    return { ok: false, error: `Choose between ${CLOSE_WAIT_MIN} and ${CLOSE_WAIT_MAX} seconds.` };
  }
  return { ok: true, value };
}

/** Add an app to the exclusion list (trimmed; duplicates ignore case, like the engine). */
export function addExcludedApp(list: readonly string[], input: string): Parsed<string[]> {
  const value = input.trim();
  if (value === "") return { ok: false, error: "Type an app name, program or app id." };
  const lower = value.toLowerCase();
  if (list.some((item) => item.trim().toLowerCase() === lower)) return { ok: false, error: `“${value}” is already on the list.` };
  if (list.length >= MAX_EXCLUDED_APPS) return { ok: false, error: `The list is full (${MAX_EXCLUDED_APPS} apps).` };
  return { ok: true, value: [...list, value] };
}

export function removeExcludedApp(list: readonly string[], value: string): string[] {
  const index = list.indexOf(value);
  return index === -1 ? [...list] : [...list.slice(0, index), ...list.slice(index + 1)];
}

// ---------------------------------------------------------------------------------------------
// This computer
// ---------------------------------------------------------------------------------------------

export const OS_LABEL: Record<OsKind, string> = {
  windows: "Windows",
  macos: "macOS",
  linux: "Linux",
  other: "Unknown system",
};

export const DISPLAY_SERVER_LABEL: Record<DisplayServer, string> = {
  win32: "Windows desktop",
  quartz: "macOS window server",
  x11: "X11",
  wayland: "Wayland",
  xWayland: "Wayland with XWayland",
  unknown: "Not detected",
};

export const SUPPORT_LABEL: Record<SupportLevel, string> = {
  full: "Full",
  partial: "Partial",
  unsupported: "Not available",
};

/** Plain names for what each platform capability lets Reframe do, in display order. */
export const CAPABILITY_NAMES: ReadonlyArray<{ key: keyof PlatformCapabilities; label: string }> = [
  { key: "listWindows", label: "See open windows" },
  { key: "readWindowGeometry", label: "Read window positions" },
  { key: "moveWindows", label: "Move and resize windows" },
  { key: "changeWindowState", label: "Maximize and minimize windows" },
  { key: "closeWindows", label: "Ask windows to close" },
  { key: "focusWindows", label: "Bring windows to the front" },
  { key: "listMonitors", label: "Detect monitors" },
  { key: "processDetails", label: "Read app folders and arguments" },
  { key: "applicationIcons", label: "Show app icons" },
];

export interface CapabilityRow {
  key: keyof PlatformCapabilities;
  label: string;
  level: SupportLevel;
  levelLabel: string;
  note: string | null;
}

export function capabilityRows(capabilities: PlatformCapabilities): CapabilityRow[] {
  return CAPABILITY_NAMES.map(({ key, label }) => {
    const capability = capabilities[key];
    const level: SupportLevel = capability?.level ?? "unsupported";
    const note = capability?.note?.trim() || null;
    return { key, label, level, levelLabel: SUPPORT_LABEL[level], note };
  });
}

/** "All 9 work fully" / "6 of 9 work fully" for the section intro. */
export function capabilitySummary(rows: readonly CapabilityRow[]): string {
  const full = rows.filter((r) => r.level === "full").length;
  if (rows.length === 0) return "";
  return full === rows.length ? `All ${rows.length} work fully here.` : `${full} of ${rows.length} work fully here.`;
}

/** "Ubuntu 24.04 · X11 · GNOME". */
export function systemDescription(info: Pick<PlatformInfo, "os" | "osVersion" | "displayServer" | "desktopEnvironment">): string {
  const parts = [
    info.osVersion.trim() || OS_LABEL[info.os],
    info.displayServer === "unknown" ? null : DISPLAY_SERVER_LABEL[info.displayServer],
    info.desktopEnvironment?.trim() || null,
  ];
  return parts.filter((p): p is string => !!p).join(" · ");
}

export const PERMISSION_LABEL: Record<PermissionKind, string> = {
  accessibility: "Accessibility",
  screenRecording: "Screen recording",
  automation: "Automation",
};
