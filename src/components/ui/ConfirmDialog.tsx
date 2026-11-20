import { useEffect, useRef, useState, type ReactNode } from "react";
import { errorMessage } from "@/lib/api";
import { Button } from "./Button";
import { Dialog } from "./Dialog";
import styles from "./ConfirmDialog.module.css";

export interface ConfirmDialogProps {
  open: boolean;
  onClose: () => void;
  /** May return a promise: the dialog shows progress and stays open until it settles. */
  onConfirm: () => void | Promise<void>;
  title: ReactNode;
  description?: ReactNode;
  children?: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  /** `danger` for destructive actions (focus starts on Cancel). */
  tone?: "default" | "danger";
}

/**
 * Ask before doing something. Errors thrown by `onConfirm` are shown inside the dialog (it stays
 * open so the user can retry or cancel); success closes it.
 */
export function ConfirmDialog({
  open,
  onClose,
  onConfirm,
  title,
  description,
  children,
  confirmLabel = "Confirm",
  cancelLabel = "Cancel",
  tone = "default",
}: ConfirmDialogProps) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // Fresh state every time the dialog opens.
  useEffect(() => {
    if (open) {
      setPending(false);
      setError(null);
    }
  }, [open]);

  const confirm = async () => {
    setError(null);
    setPending(true);
    try {
      await onConfirm();
      if (mounted.current) {
        setPending(false);
        onClose();
      }
    } catch (e) {
      if (mounted.current) {
        setPending(false);
        setError(errorMessage(e));
      }
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={title}
      description={description}
      size="sm"
      role="alertdialog"
      dismissible={!pending}
      initialFocusRef={tone === "danger" ? cancelRef : confirmRef}
      footer={
        <>
          <Button ref={cancelRef} variant="secondary" onClick={onClose} disabled={pending}>
            {cancelLabel}
          </Button>
          <Button
            ref={confirmRef}
            variant={tone === "danger" ? "danger" : "primary"}
            loading={pending}
            onClick={() => void confirm()}
          >
            {confirmLabel}
          </Button>
        </>
      }
    >
      {children || error ? (
        <>
          {children}
          {error && (
            <p role="alert" className={styles.error}>
              {error}
            </p>
          )}
        </>
      ) : undefined}
    </Dialog>
  );
}
