import { useMemo, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  ArrowRight,
  CircleCheck,
  FileDown,
  FileX,
  FolderOpen,
  FolderSearch,
  KeyRound,
  Plus,
  SquareMousePointer,
  Undo2,
  X,
} from "lucide-react";
import { Page, PageHeader } from "@/components/layout/Page";
import {
  AppIcon,
  Badge,
  Button,
  Field,
  Icon,
  IconButton,
  Input,
  SectionHeader,
  Select,
  Spinner,
  WorkspaceIcon,
  useToast,
  type BadgeTone,
  type LucideIcon,
} from "@/components/ui";
import { api, toApiError } from "@/lib/api";
import { cx } from "@/lib/cx";
import { formatShortDate, pluralize } from "@/lib/format";
import { queryClient, queryKeys } from "@/lib/queryClient";
import { navigate } from "@/lib/router";
import { Notice } from "@/features/restoration/Notice";
import { OS_LABEL } from "@/features/settings/settingsModel";
import type { ApiError, ImportApplication, ImportPreview, IconLookup, LaunchTarget } from "@/types";
import {
  MAX_NAME_LENGTH,
  addMappingRow,
  buildResolution,
  choiceFor,
  effectiveTarget,
  importCounts,
  isPathMapped,
  mappingProblems,
  mappingsForImport,
  nextRowKey,
  removeMappingRow,
  rowsFromMappings,
  selectedTargetValue,
  targetLabel,
  targetOptions,
  updateMappingRow,
  validateImportName,
  withChoice,
  type AppChoice,
  type AppChoices,
  type MappingProblem,
  type MappingRow,
} from "./importModel";
import styles from "./ImportPage.module.css";

type Step =
  | { kind: "choose" }
  | { kind: "reading"; path: string }
  | { kind: "failed"; path: string; error: ApiError }
  | { kind: "review"; preview: ImportPreview };

/** Import a workspace file exported on another computer (route `import`). */
export function ImportPage() {
  const toast = useToast();
  const [step, setStep] = useState<Step>({ kind: "choose" });

  const chooseFile = async () => {
    let path: string | null;
    try {
      path = await openDialog({
        title: "Import a Reframe workspace",
        multiple: false,
        directory: false,
        filters: [{ name: "Reframe workspace", extensions: ["json"] }],
      });
    } catch (error) {
      toast.error("Couldn't open the file picker", toApiError(error).message);
      return;
    }
    if (!path) return;
    setStep({ kind: "reading", path });
    try {
      const preview = await api.previewImport(path);
      setStep({ kind: "review", preview });
    } catch (error) {
      setStep({ kind: "failed", path, error: toApiError(error) });
    }
  };

  return (
    <Page>
      <PageHeader
        back={{ label: "Workspaces", to: { name: "workspaces" } }}
        title="Import a workspace"
        subtitle={
          step.kind === "review"
            ? "Check how it fits this computer, then import it."
            : "Bring in a workspace exported from another computer."
        }
        actions={
          step.kind === "review" ? (
            <Button variant="secondary" icon={FolderOpen} onClick={() => void chooseFile()}>
              Choose another file
            </Button>
          ) : undefined
        }
      />
      {step.kind === "review" ? (
        <ImportReview key={step.preview.filePath + step.preview.exportedAt} preview={step.preview} />
      ) : (
        <ChooseFile step={step} onChoose={() => void chooseFile()} />
      )}
    </Page>
  );
}

// ---------------------------------------------------------------------------------------------
// Choosing a file
// ---------------------------------------------------------------------------------------------

function failureCopy(error: ApiError): { title: string; text: string } {
  switch (error.kind) {
    case "invalidImport":
      return { title: "This file can't be imported", text: error.message };
    case "notImplemented":
      return {
        title: "Importing isn't available in this version yet",
        text: "This build of Reframe can't read workspace files yet. Nothing was changed.",
      };
    case "io":
      return { title: "Couldn't open the file", text: error.message };
    case "notFound":
      return { title: "The file is gone", text: "It may have been moved or deleted. Choose it again." };
    default:
      return { title: "Couldn't read this file", text: error.message };
  }
}

const FACTS: ReadonlyArray<{ icon: LucideIcon; text: string }> = [
  { icon: SquareMousePointer, text: "Each app is matched to this computer — you can pick another program or skip it." },
  { icon: FolderSearch, text: "Folders from the other computer can be mapped to where they are here." },
  { icon: KeyRound, text: "Exports never contain passwords or tokens, and nothing opens until you restore." },
];

function ChooseFile({ step, onChoose }: { step: Exclude<Step, { kind: "review" }>; onChoose: () => void }) {
  if (step.kind === "failed") {
    const copy = failureCopy(step.error);
    return (
      <div className={cx(styles.drop, styles.dropFailed)} role="alert">
        <span className={cx(styles.dropIcon, styles.dropIconDanger)}>
          <Icon icon={FileX} size={20} />
        </span>
        <div className={styles.dropCopy}>
          <h2 className={styles.dropTitle}>{copy.title}</h2>
          <p className={styles.dropText}>{copy.text}</p>
          {step.error.details.length > 1 && (
            <ul className={styles.detailList}>
              {step.error.details.map((detail) => (
                <li key={detail}>{detail}</li>
              ))}
            </ul>
          )}
          <p className={styles.dropPath} title={step.path}>
            {step.path}
          </p>
        </div>
        <div className={styles.dropActions}>
          <Button variant="secondary" onClick={() => navigate({ name: "workspaces" })}>
            Back to Workspaces
          </Button>
          <Button variant="primary" icon={FolderOpen} onClick={onChoose}>
            Choose another file
          </Button>
        </div>
      </div>
    );
  }

  const reading = step.kind === "reading";
  return (
    <div className={styles.chooseLayout}>
      <div className={styles.drop}>
        <span className={styles.dropIcon}>{reading ? <Spinner size={20} /> : <Icon icon={FileDown} size={20} />}</span>
        <div className={styles.dropCopy}>
          <h2 className={styles.dropTitle}>{reading ? "Reading the file…" : "Choose a Reframe workspace file"}</h2>
          <p className={styles.dropText}>
            {reading ? (
              <span className={styles.dropPathInline}>{step.path}</span>
            ) : (
              "Workspaces exported from Reframe are saved as .json files."
            )}
          </p>
        </div>
        <Button variant="primary" icon={FolderOpen} loading={reading} onClick={onChoose}>
          Choose file…
        </Button>
      </div>
      <ul className={styles.facts}>
        {FACTS.map((fact) => (
          <li key={fact.text} className={styles.fact}>
            <Icon icon={fact.icon} size={16} className={styles.factIcon} />
            <span>{fact.text}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Reviewing an import
// ---------------------------------------------------------------------------------------------

/** The mapping screen for a previewed import: name, apps, folders, then "Import workspace". */
export function ImportReview({ preview }: { preview: ImportPreview }) {
  const toast = useToast();
  const [name, setName] = useState(preview.name);
  const [nameTouched, setNameTouched] = useState(false);
  const [choices, setChoices] = useState<AppChoices>({});
  const [rows, setRows] = useState<MappingRow[]>(() => rowsFromMappings(preview.suggestedPathMappings));
  const [importing, setImporting] = useState(false);
  const [showProblems, setShowProblems] = useState(false);
  /** The row just added (its first field takes focus). */
  const [newRowKey, setNewRowKey] = useState<string | null>(null);
  const nameRef = useRef<HTMLInputElement>(null);
  const foldersRef = useRef<HTMLElement>(null);

  const nameError = validateImportName(name);
  const problems = useMemo(() => mappingProblems(rows), [rows]);
  const mappings = useMemo(() => mappingsForImport(rows), [rows]);
  const counts = importCounts(preview, choices);
  const missingCount = preview.applications.reduce((sum, app) => sum + app.missingPaths.length, 0);

  const updateChoice = (entryId: string, patch: Partial<AppChoice>) => setChoices((current) => withChoice(current, entryId, patch));

  const addRow = () => {
    const key = nextRowKey();
    setRows((current) => addMappingRow(current, () => key));
    setNewRowKey(key);
  };

  const pickProgram = async (app: ImportApplication) => {
    try {
      const path = await openDialog({ title: `Choose the program for ${app.displayName}`, multiple: false, directory: false });
      if (path) updateChoice(app.entryId, { target: { kind: "executable", path }, skip: false });
    } catch (error) {
      toast.error("Couldn't open the file picker", toApiError(error).message);
    }
  };

  const commit = async () => {
    setNameTouched(true);
    setShowProblems(true);
    if (nameError) {
      nameRef.current?.focus();
      return;
    }
    if (problems.size > 0) {
      foldersRef.current?.scrollIntoView({ behavior: "smooth", block: "center" });
      return;
    }
    if (counts.included === 0) return;
    setImporting(true);
    try {
      const workspace = await api.commitImport(buildResolution(preview, name, choices, rows));
      queryClient.invalidate(queryKeys.workspaces, queryKeys.activityAll);
      toast.success("Workspace imported", `${workspace.name} is ready to restore.`);
      navigate({ name: "workspace", id: workspace.id });
    } catch (error) {
      const e = toApiError(error);
      toast.error("Couldn't import the workspace", e.details.length > 0 ? e.details.join(" ") : e.message);
      setImporting(false);
    }
  };

  return (
    <div className={styles.review}>
      <section className={styles.identity} aria-label="Workspace">
        <WorkspaceIcon emoji={preview.icon} color={null} size={56} />
        <div className={styles.identityMain}>
          <Field
            label="Name"
            error={nameTouched ? nameError ?? undefined : undefined}
            counter={[...name.trim()].length > MAX_NAME_LENGTH - 10 ? { value: [...name.trim()].length, max: MAX_NAME_LENGTH } : undefined}
          >
            {(control) => (
              <Input
                {...control}
                ref={nameRef}
                value={name}
                onChange={(event) => setName(event.target.value)}
                onBlur={() => setNameTouched(true)}
              />
            )}
          </Field>
          <p className={styles.identityMeta}>
            Exported from {OS_LABEL[preview.sourceOs]}
            {formatShortDate(preview.exportedAt) && <> on {formatShortDate(preview.exportedAt)}</>}
            {preview.description && <> · {preview.description}</>}
          </p>
          <p className={styles.identityPath} title={preview.filePath}>
            {preview.filePath}
          </p>
        </div>
      </section>

      {preview.warnings.length > 0 && (
        <Notice tone="warning" title="Before you import" lines={preview.warnings} />
      )}

      <section aria-labelledby="import-apps">
        <SectionHeader
          id="import-apps"
          title="Apps"
          count={preview.applications.length}
          description="Reframe matched each app to this computer. Pick another program, or skip apps you don't need here."
        />
        {preview.applications.length === 0 ? (
          <p className={styles.emptyText}>This workspace has no apps.</p>
        ) : (
          <ul className={styles.list}>
            {preview.applications.map((app) => (
              <AppRow
                key={app.entryId}
                app={app}
                choice={choiceFor(choices, app.entryId)}
                mappings={mappings}
                onChoose={(target) => updateChoice(app.entryId, { target })}
                onPick={() => void pickProgram(app)}
                onSkip={(skip) => updateChoice(app.entryId, { skip })}
              />
            ))}
          </ul>
        )}
      </section>

      <section aria-labelledby="import-folders" ref={foldersRef}>
        <SectionHeader
          id="import-folders"
          title="Folders"
          count={mappings.length}
          description={
            missingCount > 0
              ? `${pluralize(missingCount, "folder or file", "folders or files")} from the other computer ${missingCount === 1 ? "doesn't" : "don't"} exist here. Map where they live on this computer.`
              : "Map folders from the other computer to where they live on this one."
          }
          action={
            <Button size="sm" variant="ghost" icon={Plus} onClick={addRow}>
              Add mapping
            </Button>
          }
        />
        <MappingEditor
          rows={rows}
          problems={problems}
          showIncomplete={showProblems}
          focusKey={newRowKey}
          onAdd={addRow}
          onChange={setRows}
        />
      </section>

      <div className={styles.footer}>
        <p className={styles.footerSummary} aria-live="polite">
          <span className={styles.footerStrong}>{pluralize(counts.included, "app")} will be imported</span>
          {counts.skipped > 0 && <> · {counts.skipped} skipped</>}
          {counts.attention > 0 && (
            <span className={styles.footerWarn}>
              {" "}
              · {counts.attention} without a program — {counts.attention === 1 ? "it" : "they"} won't open until you choose one
            </span>
          )}
        </p>
        <Button variant="secondary" onClick={() => navigate({ name: "workspaces" })} disabled={importing}>
          Cancel
        </Button>
        <Button variant="primary" iconEnd={ArrowRight} loading={importing} disabled={counts.included === 0} onClick={() => void commit()}>
          Import workspace
        </Button>
      </div>
    </div>
  );
}

const RESOLUTION_BADGE: Record<ImportApplication["resolution"]["kind"], { label: string; tone: BadgeTone; hint: string }> = {
  exported: { label: "Exported", tone: "success", hint: "Found at the same place as on the other computer." },
  found: { label: "Found", tone: "accent", hint: "Reframe found this app on this computer." },
  unresolved: { label: "Unresolved", tone: "warning", hint: "Reframe couldn't find this app here — choose it or skip it." },
};

interface AppRowProps {
  app: ImportApplication;
  choice: AppChoice;
  mappings: ReturnType<typeof mappingsForImport>;
  onChoose: (target: LaunchTarget | null) => void;
  onPick: () => void;
  onSkip: (skip: boolean) => void;
}

function lookupFor(app: ImportApplication, target: LaunchTarget | null): IconLookup {
  return {
    appId: app.appId,
    executablePath: target?.kind === "executable" ? target.path : null,
    package: target?.kind === "package" ? target.package : null,
  };
}

function AppRow({ app, choice, mappings, onChoose, onPick, onSkip }: AppRowProps) {
  const target = effectiveTarget(app, choice);
  const options = targetOptions(app, choice);
  const custom = choice.target !== null && selectedTargetValue(options, app, choice) !== "auto";
  const badge = choice.skip
    ? { label: "Skipped", tone: "neutral" as BadgeTone, hint: "Left out of the imported workspace." }
    : custom
      ? { label: "Chosen", tone: "accent" as BadgeTone, hint: "You chose this program." }
      : RESOLUTION_BADGE[app.resolution.kind];
  const missing = app.missingPaths;

  return (
    <li className={cx(styles.app, choice.skip && styles.appSkipped)}>
      <AppIcon lookup={lookupFor(app, target)} name={app.displayName} size={36} />
      <div className={styles.appMain}>
        <div className={styles.appHead}>
          <span className={styles.appName}>{app.displayName}</span>
          <Badge tone={badge.tone} title={badge.hint}>
            {badge.label}
          </Badge>
        </div>
        {!choice.skip && (
          <div className={styles.appTarget}>
            {options.length > 1 ? (
              <Select
                size="sm"
                className={styles.targetSelect}
                aria-label={`Program for ${app.displayName}`}
                value={selectedTargetValue(options, app, choice)}
                options={options.map((o) => ({ value: o.value, label: o.label }))}
                onChange={(value) => {
                  const option = options.find((o) => o.value === value);
                  if (option) onChoose(value === "auto" ? null : option.target);
                }}
              />
            ) : target ? (
              <span className={styles.targetPath} title={targetLabel(target)}>
                {targetLabel(target)}
              </span>
            ) : (
              <span className={styles.targetMissing}>No program chosen yet</span>
            )}
          </div>
        )}
        {!choice.skip && missing.length > 0 && (
          <ul className={styles.missing} aria-label={`Missing folders for ${app.displayName}`}>
            {missing.map((path) => {
              const mapped = isPathMapped(path, mappings);
              return (
                <li key={path} className={styles.missingItem}>
                  <span className={styles.missingPath} title={path}>
                    {path}
                  </span>
                  {mapped ? (
                    <span className={styles.missingMapped}>
                      <Icon icon={CircleCheck} size={12} /> Mapped
                    </span>
                  ) : (
                    <span className={styles.missingNote}>Not on this computer</span>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>
      <div className={styles.appActions}>
        {choice.skip ? (
          <Button size="sm" variant="secondary" icon={Undo2} onClick={() => onSkip(false)}>
            Include
          </Button>
        ) : (
          <>
            <Button size="sm" variant="secondary" onClick={onPick}>
              Choose…
            </Button>
            <Button size="sm" variant="ghost" onClick={() => onSkip(true)}>
              Skip
            </Button>
          </>
        )}
      </div>
    </li>
  );
}

interface MappingEditorProps {
  rows: readonly MappingRow[];
  problems: ReadonlyMap<string, MappingProblem>;
  /** Half-filled rows are flagged only once the user tries to import (not while typing). */
  showIncomplete: boolean;
  /** Row whose first field should take focus when it appears. */
  focusKey: string | null;
  onAdd: () => void;
  onChange: (updater: (rows: MappingRow[]) => MappingRow[]) => void;
}

function MappingEditor({ rows, problems, showIncomplete, focusKey, onAdd, onChange }: MappingEditorProps) {
  if (rows.length === 0) {
    return (
      <div className={styles.mappingEmpty}>
        <p>No folder mappings. Add one if projects or files live somewhere else on this computer.</p>
        <Button size="sm" variant="secondary" icon={Plus} onClick={onAdd}>
          Add mapping
        </Button>
      </div>
    );
  }
  return (
    <div className={styles.mappings}>
      <div className={styles.mappingHead} aria-hidden="true">
        <span>On the other computer</span>
        <span />
        <span>On this computer</span>
        <span />
      </div>
      <ul className={styles.mappingList}>
        {rows.map((row, index) => {
          const found = problems.get(row.key);
          const problem = found && (showIncomplete || found.kind === "duplicate") ? found.message : undefined;
          return (
            <li key={row.key} className={styles.mappingRow}>
              <Input
                mono
                size="sm"
                autoFocus={row.key === focusKey}
                value={row.fromPrefix}
                placeholder="C:\Users\you\Projects"
                aria-label={`Folder ${index + 1} on the other computer`}
                aria-invalid={problem ? true : undefined}
                onChange={(event) => onChange((current) => updateMappingRow(current, row.key, { fromPrefix: event.target.value }))}
              />
              <Icon icon={ArrowRight} size={14} className={styles.mappingArrow} />
              <Input
                mono
                size="sm"
                value={row.toPrefix}
                placeholder="/home/you/Projects"
                aria-label={`Folder ${index + 1} on this computer`}
                aria-invalid={problem ? true : undefined}
                onChange={(event) => onChange((current) => updateMappingRow(current, row.key, { toPrefix: event.target.value }))}
              />
              <IconButton
                icon={X}
                size="sm"
                aria-label={`Remove mapping ${index + 1}`}
                tooltip
                onClick={() => onChange((current) => removeMappingRow(current, row.key))}
              />
              {problem && (
                <p className={styles.mappingProblem} role="alert">
                  {problem}
                </p>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
