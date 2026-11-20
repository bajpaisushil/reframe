import { useCallback, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { ConfirmDialog, useToast } from "@/components/ui";
import { api, errorMessage } from "@/lib/api";
import { queryClient, queryKeys } from "@/lib/queryClient";
import type { Workspace } from "@/types";
import { exportFileName } from "../model/validation";

export interface WorkspaceRef {
  id: string;
  name: string;
}

export type WorkspaceActionKind = "duplicate" | "export" | "delete";

export interface WorkspaceActionsOptions {
  onDuplicated?: (copy: Workspace) => void;
  onDeleted?: (workspace: WorkspaceRef) => void;
}

export interface WorkspaceActions {
  /** Copy the saved workspace; resolves with the copy, or undefined when it failed (toast shown). */
  duplicate: (workspace: WorkspaceRef) => Promise<Workspace | undefined>;
  /** Ask where to save, then export. Resolves when done, cancelled or failed (toast shown). */
  exportToFile: (workspace: WorkspaceRef) => Promise<void>;
  /** Open the delete confirmation. */
  requestDelete: (workspace: WorkspaceRef) => void;
  /** Render once: the delete confirmation dialog. */
  dialogs: ReactNode;
  /** The action in progress, for spinners. */
  busy: { id: string; action: WorkspaceActionKind } | null;
}

/** Duplicate / export / delete with consistent feedback, shared by the list and detail pages. */
export function useWorkspaceActions(options: WorkspaceActionsOptions = {}): WorkspaceActions {
  const toast = useToast();
  const [busy, setBusy] = useState<WorkspaceActions["busy"]>(null);
  const [deleteTarget, setDeleteTarget] = useState<WorkspaceRef | null>(null);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const optionsRef = useRef(options);
  useLayoutEffect(() => {
    optionsRef.current = options;
  });

  const duplicate = useCallback(
    async (workspace: WorkspaceRef) => {
      setBusy({ id: workspace.id, action: "duplicate" });
      try {
        const copy = await api.duplicateWorkspace(workspace.id);
        queryClient.invalidate(queryKeys.workspaces, queryKeys.activityAll);
        toast.success("Workspace duplicated", `Saved a copy as “${copy.name}”.`);
        optionsRef.current.onDuplicated?.(copy);
        return copy;
      } catch (error) {
        toast.error(`Couldn't duplicate ${workspace.name}`, errorMessage(error));
        return undefined;
      } finally {
        setBusy(null);
      }
    },
    [toast],
  );

  const exportToFile = useCallback(
    async (workspace: WorkspaceRef) => {
      let path: string | null;
      try {
        path = await save({
          title: `Export ${workspace.name}`,
          defaultPath: exportFileName(workspace.name),
          filters: [{ name: "Reframe workspace", extensions: ["json"] }],
        });
      } catch (error) {
        toast.error("Couldn't open the save dialog", errorMessage(error));
        return;
      }
      if (!path) return;
      setBusy({ id: workspace.id, action: "export" });
      try {
        await api.exportWorkspace(workspace.id, path);
        queryClient.invalidate(queryKeys.activityAll);
        toast.success(`Exported ${workspace.name}`, path);
      } catch (error) {
        toast.error(`Couldn't export ${workspace.name}`, errorMessage(error));
      } finally {
        setBusy(null);
      }
    },
    [toast],
  );

  const requestDelete = useCallback((workspace: WorkspaceRef) => {
    setDeleteTarget(workspace);
    setDeleteOpen(true);
  }, []);

  const confirmDelete = async () => {
    if (!deleteTarget) return;
    const target = deleteTarget;
    // Errors are shown inside the confirmation (it stays open to retry or cancel).
    await api.deleteWorkspace(target.id);
    queryClient.invalidate(queryKeys.workspaces, queryKeys.activeWorkspace, queryKeys.activityAll, queryKeys.settings);
    toast.success(`Deleted ${target.name}`);
    optionsRef.current.onDeleted?.(target);
  };

  const dialogs = (
    <ConfirmDialog
      open={deleteOpen}
      onClose={() => setDeleteOpen(false)}
      onConfirm={confirmDelete}
      tone="danger"
      title={`Delete ${deleteTarget?.name ?? "workspace"}?`}
      description="This removes the workspace and its restore history from Reframe. Your apps, files and open windows aren't affected."
      confirmLabel="Delete workspace"
    />
  );

  return { duplicate, exportToFile, requestDelete, dialogs, busy };
}
