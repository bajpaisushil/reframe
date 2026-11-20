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
  useToast,
} from "@/components/ui";
import { useMutation } from "@/hooks/useQuery";
import { api, toApiError } from "@/lib/api";
import { DEFAULT_WORKSPACE_EMOJI } from "@/lib/emoji";
import { queryKeys } from "@/lib/queryClient";
import { navigate } from "@/lib/router";
import type { NewWorkspace, Workspace } from "@/types";
import styles from "./NewWorkspaceDialog.module.css";

/** Limits enforced by the backend (`Workspace::validate`). */
const MAX_NAME = 80;
const MAX_DESCRIPTION = 500;

export interface NewWorkspaceDialogProps {
  open: boolean;
  onClose: () => void;
}

/** Create an empty workspace (apps are added later from Explore), then open it. */
export function NewWorkspaceDialog({ open, onClose }: NewWorkspaceDialogProps) {
  const toast = useToast();
  const [name, setName] = useState("");
  const [icon, setIcon] = useState(DEFAULT_WORKSPACE_EMOJI);
  const [color, setColor] = useState<string | null>(null);
  const [description, setDescription] = useState("");
  const [pickerOpen, setPickerOpen] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);

  const create = useMutation<NewWorkspace, Workspace>((input) => api.createWorkspace(input), {
    invalidates: [queryKeys.workspaces, queryKeys.activityAll],
  });

  // Start fresh each time the dialog opens.
  useEffect(() => {
    if (!open) return;
    setName("");
    setIcon(DEFAULT_WORKSPACE_EMOJI);
    setColor(null);
    setDescription("");
    setPickerOpen(false);
    setSubmitted(false);
    create.reset();
    // `create.reset` is stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const trimmedName = name.trim();
  const nameError =
    submitted && trimmedName === ""
      ? "Give your workspace a name."
      : [...trimmedName].length > MAX_NAME
        ? `Use at most ${MAX_NAME} characters.`
        : undefined;
  const descriptionTooLong = [...description.trim()].length > MAX_DESCRIPTION;
  const serverError = create.error ? toApiError(create.error) : null;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setSubmitted(true);
    if (trimmedName === "" || [...trimmedName].length > MAX_NAME || descriptionTooLong) {
      nameRef.current?.focus();
      return;
    }
    try {
      const workspace = await create.mutateAsync({
        name: trimmedName,
        icon,
        color,
        description: description.trim(),
      });
      toast.success("Workspace created", `${workspace.name} is ready. Add apps to it from Explore.`);
      onClose();
      navigate({ name: "workspace", id: workspace.id });
    } catch {
      // Shown inline below (create.error); the dialog stays open so nothing typed is lost.
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="New workspace"
      description="Name it now — add apps from Explore any time."
      dismissible={!create.isPending}
      initialFocusRef={nameRef}
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={create.isPending}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="new-workspace-form" loading={create.isPending}>
            Create workspace
          </Button>
        </>
      }
    >
      <form id="new-workspace-form" className={styles.form} onSubmit={(event) => void submit(event)} noValidate>
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
          <Field label="Name" error={nameError} counter={name.length > MAX_NAME - 20 ? { value: [...name].length, max: MAX_NAME } : undefined} className={styles.name}>
            {(control) => (
              <Input
                {...control}
                ref={nameRef}
                value={name}
                placeholder="e.g. Deep work"
                onChange={(event) => setName(event.target.value)}
                autoComplete="off"
                spellCheck={false}
              />
            )}
          </Field>
        </div>

        <div className={styles.row}>
          <span className={styles.label} id="new-workspace-color">
            Color
          </span>
          <ColorPicker value={color} onChange={setColor} aria-label="Color" />
        </div>

        <Field
          label="Description"
          optional
          error={descriptionTooLong ? `Use at most ${MAX_DESCRIPTION} characters.` : undefined}
          counter={description.length > MAX_DESCRIPTION - 80 ? { value: [...description].length, max: MAX_DESCRIPTION } : undefined}
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

        {serverError && (
          <div role="alert" className={styles.error}>
            <p className={styles.errorTitle}>Couldn&rsquo;t create the workspace</p>
            {serverError.details.length > 1 ? (
              <ul className={styles.errorList}>
                {serverError.details.map((detail) => (
                  <li key={detail}>{detail}</li>
                ))}
              </ul>
            ) : (
              <p>{serverError.message}</p>
            )}
          </div>
        )}
      </form>
    </Dialog>
  );
}
