import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { AlertTriangle, Check, ChevronDown } from "lucide-react";
import {
  AppIconStack,
  Button,
  ColorPicker,
  Dialog,
  EmojiPicker,
  Field,
  Icon,
  Input,
  Popover,
  SegmentedControl,
  Textarea,
  WorkspaceIcon,
  useToast,
} from "@/components/ui";
import { useMutation } from "@/hooks/useQuery";
import { useActiveWorkspaceId, useWorkspaces } from "@/hooks/useWorkspaces";
import { api, toApiError } from "@/lib/api";
import { DEFAULT_WORKSPACE_EMOJI } from "@/lib/emoji";
import { pluralize } from "@/lib/format";
import { queryKeys } from "@/lib/queryClient";
import { navigate } from "@/lib/router";
import { sortByRecentUse } from "@/lib/workspaces";
import type { ApiError, CaptureSelection, DiscoveredApplication, Workspace } from "@/types";
import { appendCapture } from "./capture";
import { iconLookup } from "./describe";
import { formatCounts, type SelectionCounts } from "./selection";
import styles from "./SaveWorkspaceDialog.module.css";

/** Limits enforced by the backend (`Workspace::validate`). */
const MAX_NAME = 80;
const MAX_DESCRIPTION = 500;

type Mode = "new" | "existing";

interface SaveInput {
  mode: Mode;
  name: string;
  icon: string;
  color: string | null;
  description: string;
  targetId: string | null;
  selections: CaptureSelection[];
}

interface Outcome {
  mode: Mode;
  workspace: Workspace;
  /** Applications saved by this capture. */
  added: number;
  warnings: string[];
}

/** Capture into a new workspace, or append fresh captures to an existing one. */
async function save(input: SaveInput): Promise<Outcome> {
  if (input.mode === "new") {
    const captured = await api.captureWorkspace({
      name: input.name,
      icon: input.icon,
      color: input.color,
      description: input.description,
      selections: input.selections,
    });
    return {
      mode: "new",
      workspace: captured.workspace,
      added: captured.workspace.applications.length,
      warnings: captured.warnings,
    };
  }
  if (!input.targetId) throw { kind: "validation", message: "Choose a workspace to add to.", details: [] } satisfies ApiError;
  const result = await api.captureApplications(input.selections);
  if (result.entries.length === 0) {
    const message = "None of the selected applications could be captured.";
    throw { kind: "validation", message, details: [message, ...result.warnings] } satisfies ApiError;
  }
  const current = await api.getWorkspace(input.targetId);
  const workspace = await api.updateWorkspace(appendCapture(current, result));
  return { mode: "existing", workspace, added: result.entries.length, warnings: result.warnings };
}

export interface SaveWorkspaceDialogProps {
  open: boolean;
  onClose: () => void;
  selections: CaptureSelection[];
  /** The selected applications (for the summary). */
  apps: readonly DiscoveredApplication[];
  counts: SelectionCounts;
  /** After a successful save (before any navigation). */
  onSaved?: (workspace: Workspace) => void;
}

/** "Save as workspace…": name a new workspace (emoji, color, description) or pick one to add to. */
export function SaveWorkspaceDialog({ open, onClose, selections, apps, counts, onSaved }: SaveWorkspaceDialogProps) {
  const toast = useToast();
  const workspaces = useWorkspaces();
  const activeId = useActiveWorkspaceId();
  const [mode, setMode] = useState<Mode>("new");
  const [name, setName] = useState("");
  const [icon, setIcon] = useState(DEFAULT_WORKSPACE_EMOJI);
  const [color, setColor] = useState<string | null>(null);
  const [description, setDescription] = useState("");
  const [targetId, setTargetId] = useState<string | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const nameRef = useRef<HTMLInputElement>(null);
  const openButtonRef = useRef<HTMLButtonElement>(null);

  const mutation = useMutation<SaveInput, Outcome>(save, {
    invalidates: (result) => [queryKeys.workspaces, queryKeys.workspace(result.workspace.id), queryKeys.activityAll],
  });

  const targets = useMemo(() => sortByRecentUse(workspaces.data ?? []), [workspaces.data]);
  const canAppend = targets.length > 0;

  // Start fresh each time the dialog opens.
  useEffect(() => {
    if (!open) return;
    setMode("new");
    setName("");
    setIcon(DEFAULT_WORKSPACE_EMOJI);
    setColor(null);
    setDescription("");
    setTargetId(null);
    setPickerOpen(false);
    setSubmitted(false);
    setOutcome(null);
    mutation.reset();
    // `mutation.reset` is stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // The form (and the focused button) is replaced by the notes: move focus to the next step.
  useEffect(() => {
    if (outcome) openButtonRef.current?.focus({ preventScroll: true });
  }, [outcome]);

  // Default target: the active workspace, else the most recently used one.
  const target = targets.find((w) => w.id === targetId) ?? targets.find((w) => w.id === activeId.data) ?? targets[0];

  const trimmedName = name.trim();
  const nameLength = [...trimmedName].length;
  const nameError =
    submitted && mode === "new" && trimmedName === ""
      ? "Give your workspace a name."
      : nameLength > MAX_NAME
        ? `Use at most ${MAX_NAME} characters.`
        : undefined;
  const descriptionTooLong = [...description.trim()].length > MAX_DESCRIPTION;
  const serverError = mutation.error ? toApiError(mutation.error) : null;
  const busy = mutation.isPending;

  const finish = (result: Outcome) => {
    onClose();
    navigate({ name: "workspace", id: result.workspace.id });
  };

  const submit = async (event?: FormEvent) => {
    event?.preventDefault();
    setSubmitted(true);
    if (selections.length === 0) return;
    if (mode === "new" && (trimmedName === "" || nameLength > MAX_NAME || descriptionTooLong)) {
      nameRef.current?.focus();
      return;
    }
    let result: Outcome;
    try {
      result = await mutation.mutateAsync({
        mode,
        name: trimmedName,
        icon,
        color,
        description: description.trim(),
        targetId: target?.id ?? null,
        selections,
      });
    } catch {
      // Shown inline (mutation.error); the dialog stays open so nothing typed is lost.
      return;
    }
    onSaved?.(result.workspace);
    if (result.warnings.length > 0) {
      // Saved, but some things need the user's attention: show them before moving on.
      setOutcome(result);
      return;
    }
    toast.success(
      result.mode === "new" ? "Workspace saved" : `Added to ${result.workspace.name}`,
      result.mode === "new"
        ? `${result.workspace.name} · ${pluralize(result.added, "app")}`
        : `${pluralize(result.added, "app")} added.`,
    );
    finish(result);
  };

  if (outcome) {
    return (
      <Dialog
        open={open}
        onClose={onClose}
        title={outcome.mode === "new" ? "Saved, with a few notes" : `Added to ${outcome.workspace.name}, with a few notes`}
        description={
          outcome.mode === "new"
            ? `${outcome.workspace.name} is saved with ${pluralize(outcome.added, "app")}. A few things didn't come through exactly:`
            : `${pluralize(outcome.added, "app")} added. A few things didn't come through exactly:`
        }
        footer={
          <>
            <Button variant="secondary" onClick={onClose}>
              Stay here
            </Button>
            <Button ref={openButtonRef} variant="primary" onClick={() => finish(outcome)}>
              Open workspace
            </Button>
          </>
        }
      >
        <ul className={styles.warnings}>
          {outcome.warnings.map((warning, index) => (
            <li key={`${index}-${warning}`} className={styles.warning}>
              <Icon icon={AlertTriangle} size={16} className={styles.warningIcon} />
              <span>{warning}</span>
            </li>
          ))}
        </ul>
      </Dialog>
    );
  }

  const summaryApps = apps.map((app) => ({ lookup: iconLookup(app), name: app.displayName }));

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Save as workspace"
      description="Reframe remembers these apps, what's open in them and where their windows are."
      dismissible={!busy}
      initialFocusRef={nameRef}
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button
            variant="primary"
            type="submit"
            form="save-workspace-form"
            loading={busy}
            disabled={selections.length === 0 || (mode === "existing" && !target)}
          >
            {mode === "new" ? "Save workspace" : "Add to workspace"}
          </Button>
        </>
      }
    >
      <form id="save-workspace-form" className={styles.form} onSubmit={(event) => void submit(event)} noValidate>
        <div className={styles.summary}>
          <AppIconStack apps={summaryApps} max={7} size={20} />
          <span className={styles.summaryText}>{formatCounts(counts)}</span>
        </div>

        {canAppend && (
          <SegmentedControl
            aria-label="Where to save"
            value={mode}
            onChange={(value) => {
              setMode(value);
              mutation.reset();
            }}
            options={[
              { value: "new", label: "New workspace" },
              { value: "existing", label: "Add to existing" },
            ]}
            fullWidth
          />
        )}

        {mode === "new" ? (
          <>
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
                counter={name.length > MAX_NAME - 20 ? { value: [...name].length, max: MAX_NAME } : undefined}
                className={styles.name}
              >
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
              <span className={styles.label}>Color</span>
              <ColorPicker value={color} onChange={setColor} aria-label="Color" />
            </div>

            <Field
              label="Description"
              optional
              error={descriptionTooLong ? `Use at most ${MAX_DESCRIPTION} characters.` : undefined}
              counter={
                description.length > MAX_DESCRIPTION - 80 ? { value: [...description].length, max: MAX_DESCRIPTION } : undefined
              }
            >
              {(control) => (
                <Textarea
                  {...control}
                  value={description}
                  rows={2}
                  placeholder="What is this workspace for?"
                  onChange={(event) => setDescription(event.target.value)}
                />
              )}
            </Field>
          </>
        ) : (
          <fieldset className={styles.targets}>
            <legend className={styles.label}>Add to</legend>
            <div className={styles.targetList}>
              {targets.map((workspace) => {
                const checked = workspace.id === target?.id;
                return (
                  <label key={workspace.id} className={styles.target} data-checked={checked || undefined}>
                    <input
                      type="radio"
                      name="save-target"
                      className={styles.radio}
                      value={workspace.id}
                      checked={checked}
                      onChange={() => setTargetId(workspace.id)}
                    />
                    <WorkspaceIcon emoji={workspace.icon} color={workspace.color} size={28} />
                    <span className={styles.targetText}>
                      <span className={styles.targetName}>{workspace.name}</span>
                      <span className={styles.targetMeta}>
                        {workspace.applicationCount === 0 ? "No apps yet" : pluralize(workspace.applicationCount, "app")}
                      </span>
                    </span>
                    {checked && <Icon icon={Check} size={16} className={styles.targetCheck} />}
                  </label>
                );
              })}
            </div>
            <p className={styles.hint}>The selected apps are added after the ones already in it.</p>
          </fieldset>
        )}

        {serverError && (
          <div role="alert" className={styles.error}>
            <p className={styles.errorTitle}>
              {mode === "new" ? "Couldn't save the workspace" : "Couldn't add the apps"}
            </p>
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
