import { useState } from "react";
import { Button, Dialog } from "@/components/ui";
import styles from "./LeaveDialog.module.css";

export interface LeaveDialogProps {
  open: boolean;
  workspaceName: string;
  /** Stay on the page. */
  onStay: () => void;
  /** Leave and lose the changes. */
  onDiscard: () => void;
  /** Save, then leave; resolves false when saving failed (the page shows why). */
  onSave: () => Promise<boolean>;
}

/** "Leave without saving?" — Discard / Keep editing / Save and leave. */
export function LeaveDialog({ open, workspaceName, onStay, onDiscard, onSave }: LeaveDialogProps) {
  const [saving, setSaving] = useState(false);
  const save = async () => {
    setSaving(true);
    try {
      await onSave();
    } finally {
      setSaving(false);
    }
  };
  return (
    <Dialog
      open={open}
      onClose={onStay}
      size="md"
      role="alertdialog"
      dismissible={!saving}
      title="Leave without saving?"
      description={`Your changes to ${workspaceName} haven't been saved yet. If you leave now, they'll be lost.`}
      footer={
        <>
          <Button variant="ghost" className={styles.discard} onClick={onDiscard} disabled={saving}>
            Discard changes
          </Button>
          <Button variant="secondary" onClick={onStay} disabled={saving} data-autofocus="true">
            Keep editing
          </Button>
          <Button variant="primary" onClick={() => void save()} loading={saving}>
            Save and leave
          </Button>
        </>
      }
    />
  );
}
