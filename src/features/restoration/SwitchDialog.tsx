// Shared contract: restore / switch to a workspace, letting the user choose Replace, Merge or
// Additive and (for Replace) confirm which windows may be closed.
//
// PLACEHOLDER: minimal stub with the final props (restores with the default mode, closes nothing). The
// restoration work replaces it with the full dialog (switch plan, close confirmation, warnings).
import type { SwitchMode } from "@/types";
import { Button, Dialog } from "@/components/ui";
import { useRestoreWorkspace } from "@/hooks/useRestoreWorkspace";

export interface SwitchDialogProps {
  open: boolean;
  onClose: () => void;
  workspace: { id: string; name: string; icon: string; color: string | null };
  /** Pre-selected mode (defaults to the user's default switch mode from settings). */
  defaultMode?: SwitchMode;
}

export function SwitchDialog({ open, onClose, workspace }: SwitchDialogProps) {
  const { restore } = useRestoreWorkspace();
  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={`Restore ${workspace.name}`}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={() => {
              onClose();
              void restore(workspace);
            }}
          >
            Restore
          </Button>
        </>
      }
    />
  );
}
