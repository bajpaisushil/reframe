// Shared contract: pick running applications (and optionally specific windows) from the current
// desktop. Used by the workspace page ("Add applications", "Update from current desktop").
//
// PLACEHOLDER: minimal stub with the final props. The Explore/capture work replaces the body with
// the real picker (scan, app list with windows, manual "Choose an app…").
import type { ApplicationEntry, CaptureSelection } from "@/types";
import { Button, Dialog } from "@/components/ui";

export interface AppPickerDialogProps {
  open: boolean;
  onClose: () => void;
  /** e.g. "Add applications" or "Update from current desktop". */
  title: string;
  /** Primary button label; the picker may append a count ("Add 3 apps"). */
  confirmLabel: string;
  /** Called with the chosen applications/windows; the caller decides what to do with them. */
  onConfirm: (selections: CaptureSelection[]) => Promise<void> | void;
  /** When provided, the picker offers "Choose an app manually…" (executable / app bundle). */
  onManualEntry?: (entry: ApplicationEntry) => void;
}

export function AppPickerDialog({ open, onClose, title }: AppPickerDialogProps) {
  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={title}
      description="Choosing applications from your desktop is coming next."
      footer={<Button onClick={onClose}>Close</Button>}
    />
  );
}
