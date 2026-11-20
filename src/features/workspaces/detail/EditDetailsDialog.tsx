import { useEffect, useRef, useState, type FormEvent } from "react";
import { ChevronDown } from "lucide-react";
import {
  Button,
  ColorPicker,
  Dialog,
  EmojiPicker,
  Field,
  Icon,
  Input,
  Popover,
  Textarea,
  WorkspaceIcon,
} from "@/components/ui";
import { toApiError } from "@/lib/api";
import { DEFAULT_WORKSPACE_EMOJI } from "@/lib/emoji";
import type { ApiError } from "@/types";
import type { DetailsPatch } from "../model/draft";
import styles from "./EditDetailsDialog.module.css";

/** Limits enforced by the backend (`Workspace::validate`). */
const MAX_NAME = 80;
const MAX_DESCRIPTION = 500;

export interface WorkspaceDetails {
  name: string;
  icon: string;
  color: string | null;
  description: string;
}

export interface EditDetailsDialogProps {
  open: boolean;
  onClose: () => void;
  initial: WorkspaceDetails;
  /** Apply and save; reject to keep the dialog open with the error. */
  onSave: (details: Required<DetailsPatch>) => Promise<void>;
}

const length = (value: string) => [...value].length;

/** Rename the workspace and change its icon, color and description. */
export function EditDetailsDialog({ open, onClose, initial, onSave }: EditDetailsDialogProps) {
  const [name, setName] = useState(initial.name);
  const [icon, setIcon] = useState(initial.icon);
  const [color, setColor] = useState<string | null>(initial.color);
  const [description, setDescription] = useState(initial.description);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<ApiError | null>(null);
  const nameRef = useRef<HTMLInputElement>(null);

  // Start from the current details every time the dialog opens.
  useEffect(() => {
    if (!open) return;
    setName(initial.name);
    setIcon(initial.icon || DEFAULT_WORKSPACE_EMOJI);
    setColor(initial.color);
    setDescription(initial.description);
    setPickerOpen(false);
    setSubmitted(false);
    setSaving(false);
    setError(null);
    // Only when opening: edits in progress must not be overwritten by a background refresh.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const trimmedName = name.trim();
  const nameError =
    submitted && trimmedName === ""
      ? "Give your workspace a name."
      : length(trimmedName) > MAX_NAME
        ? `Use at most ${MAX_NAME} characters.`
        : undefined;
  const descriptionTooLong = length(description.trim()) > MAX_DESCRIPTION;
  const unchanged =
    trimmedName === initial.name && icon === initial.icon && color === initial.color && description.trim() === initial.description;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setSubmitted(true);
    if (trimmedName === "" || length(trimmedName) > MAX_NAME || descriptionTooLong) {
      nameRef.current?.focus();
      return;
    }
    if (unchanged) {
      onClose();
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await onSave({ name: trimmedName, icon, color, description: description.trim() });
      onClose();
    } catch (e) {
      setError(toApiError(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Edit details"
      description="How this workspace appears in Reframe and the tray menu."
      dismissible={!saving}
      initialFocusRef={nameRef}
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={saving}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="edit-workspace-form" loading={saving}>
            Save
          </Button>
        </>
      }
    >
      <form id="edit-workspace-form" className={styles.form} onSubmit={(event) => void submit(event)} noValidate>
        <div className={styles.identity}>
          <Popover
            open={pickerOpen}
            onOpenChange={setPickerOpen}
            aria-label="Choose an icon"
            placement="bottom-start"
            trigger={(props) => (
              <button {...props} type="button" className={styles.iconButton} aria-label={`Icon: ${icon}. Change icon`}>
                <WorkspaceIcon emoji={icon} color={color} size={44} />
                <span className={styles.iconChevron} aria-hidden="true">
                  <Icon icon={ChevronDown} size={12} />
                </span>
              </button>
            )}
          >
            {(close) => (
              <EmojiPicker
                value={icon}
                onChange={(emoji) => {
                  setIcon(emoji);
                  close();
                }}
              />
            )}
          </Popover>
          <Field
            label="Name"
            error={nameError}
            counter={length(name) > MAX_NAME - 20 ? { value: length(name), max: MAX_NAME } : undefined}
            className={styles.name}
          >
            {(control) => (
              <Input
                {...control}
                ref={nameRef}
                value={name}
                placeholder="e.g. Deep work"
                onChange={(event) => setName(event.target.value)}
                spellCheck={false}
              />
            )}
          </Field>
        </div>

        <div className={styles.row}>
          <span className={styles.label}>Color</span>
          <ColorPicker value={color} onChange={setColor} aria-label="Color" />
        </div>

        <Field
          label="Description"
          optional
          error={descriptionTooLong ? `Use at most ${MAX_DESCRIPTION} characters.` : undefined}
          counter={length(description) > MAX_DESCRIPTION - 80 ? { value: length(description), max: MAX_DESCRIPTION } : undefined}
        >
          {(control) => (
            <Textarea
              {...control}
              value={description}
              rows={3}
              placeholder="What is this workspace for?"
              onChange={(event) => setDescription(event.target.value)}
            />
          )}
        </Field>

        {error && (
          <div role="alert" className={styles.error}>
            <p className={styles.errorTitle}>Couldn&rsquo;t save the changes</p>
            {error.details.length > 1 ? (
              <ul className={styles.errorList}>
                {error.details.map((detail) => (
                  <li key={detail}>{detail}</li>
                ))}
              </ul>
            ) : (
              <p>{error.message}</p>
            )}
          </div>
        )}
      </form>
    </Dialog>
  );
}
