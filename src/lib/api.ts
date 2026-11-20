// Typed client for every Reframe Tauri command. Components must use this module rather than
// calling `invoke` directly, so command names and payloads stay in one place.
import { invoke } from "@tauri-apps/api/core";
import type {
  ActivityEntry,
  ApiError,
  AppSettings,
  ApplicationEntry,
  CaptureRequest,
  CaptureResult,
  CaptureSelection,
  CapturedWorkspace,
  DesktopSnapshot,
  IconLookup,
  ImportPreview,
  ImportResolution,
  LaunchTarget,
  NewWorkspace,
  PermissionKind,
  PermissionStatus,
  PlatformInfo,
  RestoreRequest,
  RestoreRun,
  StartupContext,
  SwitchMode,
  SwitchPlan,
  Workspace,
  WorkspaceHealth,
  WorkspaceSummary,
} from "@/types";

export const api = {
  // Platform & desktop
  platformInfo: () => invoke<PlatformInfo>("get_platform_info"),
  permissions: () => invoke<PermissionStatus[]>("get_permissions"),
  requestPermission: (kind: PermissionKind) => invoke<void>("request_permission", { kind }),
  scanDesktop: () => invoke<DesktopSnapshot>("scan_desktop"),
  appIcon: (lookup: IconLookup) => invoke<string | null>("get_app_icon", { lookup }),

  // Workspaces
  listWorkspaces: () => invoke<WorkspaceSummary[]>("list_workspaces"),
  getWorkspace: (id: string) => invoke<Workspace>("get_workspace", { id }),
  createWorkspace: (input: NewWorkspace) => invoke<Workspace>("create_workspace", { input }),
  captureWorkspace: (request: CaptureRequest) => invoke<CapturedWorkspace>("capture_workspace", { request }),
  recaptureWorkspace: (id: string, selections: CaptureSelection[]) =>
    invoke<CapturedWorkspace>("recapture_workspace", { id, selections }),
  captureApplications: (selections: CaptureSelection[]) =>
    invoke<CaptureResult>("capture_applications", { selections }),
  createManualEntry: (target: LaunchTarget) => invoke<ApplicationEntry>("create_manual_entry", { target }),
  updateWorkspace: (workspace: Workspace) => invoke<Workspace>("update_workspace", { workspace }),
  deleteWorkspace: (id: string) => invoke<void>("delete_workspace", { id }),
  duplicateWorkspace: (id: string) => invoke<Workspace>("duplicate_workspace", { id }),
  workspaceHealth: (id: string) => invoke<WorkspaceHealth>("get_workspace_health", { id }),

  // Switching & restore (progress: see onRestoreEvent in ./events)
  planSwitch: (workspaceId: string, mode: SwitchMode) => invoke<SwitchPlan>("plan_switch", { workspaceId, mode }),
  startRestore: (request: RestoreRequest) => invoke<string>("start_restore", { request }),
  cancelRestore: (runId: string) => invoke<void>("cancel_restore", { runId }),
  activeRuns: () => invoke<RestoreRun[]>("get_active_runs"),
  restoreRun: (runId: string) => invoke<RestoreRun>("get_restore_run", { runId }),
  restoreHistory: (workspaceId: string | null, limit: number) =>
    invoke<RestoreRun[]>("list_restore_runs", { workspaceId, limit }),

  // Activity, settings, startup
  activity: (limit: number) => invoke<ActivityEntry[]>("get_activity", { limit }),
  settings: () => invoke<AppSettings>("get_settings"),
  updateSettings: (settings: AppSettings) => invoke<AppSettings>("update_settings", { settings }),
  activeWorkspace: () => invoke<string | null>("get_active_workspace"),
  startupContext: () => invoke<StartupContext>("get_startup_context"),
  dismissStartupRestore: () => invoke<void>("dismiss_startup_restore"),

  // Import / export (paths come from the dialog plugin)
  exportWorkspace: (id: string, path: string) => invoke<void>("export_workspace", { id, path }),
  previewImport: (path: string) => invoke<ImportPreview>("preview_import", { path }),
  commitImport: (resolution: ImportResolution) => invoke<Workspace>("commit_import", { resolution }),
};

/** True when running inside the Tauri webview (false in a plain browser tab). */
export const isTauri = (): boolean => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Normalize anything thrown by `invoke` into an ApiError. */
export function toApiError(error: unknown): ApiError {
  if (error && typeof error === "object" && "message" in error && "kind" in error) {
    const e = error as Partial<ApiError>;
    return { kind: (e.kind as ApiError["kind"]) ?? "internal", message: String(e.message), details: e.details ?? [] };
  }
  return { kind: "internal", message: typeof error === "string" ? error : String(error), details: [] };
}

/** Human-readable message for any thrown error. */
export function errorMessage(error: unknown): string {
  const e = toApiError(error);
  return e.details.length > 1 ? `${e.message}` : e.message;
}
