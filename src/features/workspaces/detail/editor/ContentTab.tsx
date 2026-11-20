import { useId, useState, type FormEvent } from "react";
import { FileCode, FilePlus, FileText, Folder, FolderPlus, Globe, Pin, Plus, ShieldAlert, Trash, X } from "lucide-react";
import { Badge, Button, Field, Icon, IconButton, Input, Select, Switch, useToast } from "@/components/ui";
import { errorMessage } from "@/lib/api";
import { cx } from "@/lib/cx";
import { pluralize } from "@/lib/format";
import type { AppState, ApplicationEntry, EntryHealth, TerminalSession } from "@/types";
import { PathPicker } from "../../components/PathPicker";
import { PathText } from "../../components/PathText";
import { pickFiles, pickFolder } from "../../components/pickers";
import {
  addBrowserTab,
  emptyTerminalSession,
  pathLeaf,
  removeAt,
  removeBrowserTab,
  replaceAt,
  updateTerminalSession,
} from "../../model/entries";
import { parsePage, parseTabAddress } from "../../model/validation";
import styles from "./editor.module.css";

type StateOf<K extends AppState["kind"]> = Extract<AppState, { kind: K }>;

export interface ContentTabProps {
  entry: ApplicationEntry;
  health: EntryHealth | undefined;
  onStateChange: (state: AppState) => void;
}

/** What the app reopens, edited according to the kind of state its integration saves. */
export function ContentTab({ entry, health, onStateChange }: ContentTabProps) {
  const missing = new Set(health?.missingPaths ?? []);
  const name = entry.displayName;
  const state = entry.state;
  switch (state.kind) {
    case "projects":
      return <ProjectsEditor state={state} missing={missing} appName={name} onChange={onStateChange} />;
    case "browser":
      return <BrowserEditor state={state} onChange={onStateChange} />;
    case "terminal":
      return <TerminalEditor state={state} missing={missing} appName={name} onChange={onStateChange} />;
    case "folders":
      return <FoldersEditor state={state} missing={missing} appName={name} onChange={onStateChange} />;
    case "documents":
      return <DocumentsEditor state={state} missing={missing} appName={name} onChange={onStateChange} />;
    case "media":
      return <MediaEditor state={state} onChange={onStateChange} />;
    case "custom":
      return (
        <div className={styles.group}>
          <p className={styles.hint}>Saved by the app&rsquo;s integration and restored as is. It can&rsquo;t be edited here.</p>
          <pre className={cx(styles.code, "selectable")}>{JSON.stringify(state.data, null, 2)}</pre>
        </div>
      );
    default:
      return null;
  }
}

function MissingBadge() {
  return (
    <Badge tone="warning" className={styles.inlineBadge}>
      Not found
    </Badge>
  );
}

// ---- Projects ----------------------------------------------------------------------------------

function ProjectsEditor({
  state,
  missing,
  appName,
  onChange,
}: {
  state: StateOf<"projects">;
  missing: ReadonlySet<string>;
  appName: string;
  onChange: (state: AppState) => void;
}) {
  const toast = useToast();
  const add = async () => {
    try {
      const path = await pickFolder(`Add a project folder to ${appName}`);
      if (!path) return;
      if (state.projects.some((p) => p.path === path)) {
        toast.info("That folder is already in the list");
        return;
      }
      onChange({ ...state, projects: [...state.projects, { path, isWorkspaceFile: false, windowTitle: null }] });
    } catch (error) {
      toast.error("Couldn't open the folder picker", errorMessage(error));
    }
  };
  return (
    <div className={styles.group}>
      <p className={styles.hint}>{appName} reopens these folders, each in its own window.</p>
      {state.projects.length === 0 ? (
        <div className={styles.empty}>No project folders yet — {appName} will open without one.</div>
      ) : (
        <ul className={styles.list}>
          {state.projects.map((project, index) => (
            <li key={`${project.path}-${index}`} className={styles.listRow}>
              <Icon icon={project.isWorkspaceFile ? FileCode : Folder} size={16} className={styles.listIcon} />
              <div className={styles.listText}>
                <PathText path={project.path} />
              </div>
              {project.isWorkspaceFile && <Badge className={styles.inlineBadge}>Workspace file</Badge>}
              {missing.has(project.path) && <MissingBadge />}
              <IconButton
                icon={X}
                size="sm"
                aria-label={`Remove ${pathLeaf(project.path)}`}
                className={styles.removeButton}
                onClick={() => onChange({ ...state, projects: removeAt(state.projects, index) })}
              />
            </li>
          ))}
        </ul>
      )}
      <div className={styles.actions}>
        <Button size="sm" variant="secondary" icon={FolderPlus} onClick={() => void add()}>
          Add folder…
        </Button>
      </div>
    </div>
  );
}

// ---- Folders -----------------------------------------------------------------------------------

function FoldersEditor({
  state,
  missing,
  appName,
  onChange,
}: {
  state: StateOf<"folders">;
  missing: ReadonlySet<string>;
  appName: string;
  onChange: (state: AppState) => void;
}) {
  const toast = useToast();
  const add = async () => {
    try {
      const path = await pickFolder(`Add a folder to open in ${appName}`);
      if (!path) return;
      if (state.folders.some((f) => f.path === path)) {
        toast.info("That folder is already in the list");
        return;
      }
      onChange({ ...state, folders: [...state.folders, { path, windowTitle: null }] });
    } catch (error) {
      toast.error("Couldn't open the folder picker", errorMessage(error));
    }
  };
  return (
    <div className={styles.group}>
      <p className={styles.hint}>{appName} opens a window for each of these folders.</p>
      {state.folders.length === 0 ? (
        <div className={styles.empty}>No folders yet.</div>
      ) : (
        <ul className={styles.list}>
          {state.folders.map((folder, index) => (
            <li key={`${folder.path}-${index}`} className={styles.listRow}>
              <Icon icon={Folder} size={16} className={styles.listIcon} />
              <div className={styles.listText}>
                <PathText path={folder.path} />
              </div>
              {missing.has(folder.path) && <MissingBadge />}
              <IconButton
                icon={X}
                size="sm"
                aria-label={`Remove ${pathLeaf(folder.path)}`}
                className={styles.removeButton}
                onClick={() => onChange({ ...state, folders: removeAt(state.folders, index) })}
              />
            </li>
          ))}
        </ul>
      )}
      <div className={styles.actions}>
        <Button size="sm" variant="secondary" icon={FolderPlus} onClick={() => void add()}>
          Add folder…
        </Button>
      </div>
    </div>
  );
}

// ---- Documents ---------------------------------------------------------------------------------

function DocumentsEditor({
  state,
  missing,
  appName,
  onChange,
}: {
  state: StateOf<"documents">;
  missing: ReadonlySet<string>;
  appName: string;
  onChange: (state: AppState) => void;
}) {
  const toast = useToast();
  const add = async () => {
    try {
      const paths = await pickFiles(`Add documents to open in ${appName}`);
      const fresh = paths.filter((path) => !state.documents.some((d) => d.path === path));
      if (fresh.length === 0) return;
      onChange({ ...state, documents: [...state.documents, ...fresh.map((path) => ({ path, page: null }))] });
    } catch (error) {
      toast.error("Couldn't open the file picker", errorMessage(error));
    }
  };
  return (
    <div className={styles.group}>
      <p className={styles.hint}>{appName} opens these files{state.documents.some((d) => d.page !== null) ? ", at the saved page where it can" : ""}.</p>
      {state.documents.length === 0 ? (
        <div className={styles.empty}>No documents yet.</div>
      ) : (
        <ul className={styles.list}>
          {state.documents.map((document, index) => (
            <li key={`${document.path}-${index}`} className={styles.listRow}>
              <Icon icon={FileText} size={16} className={styles.listIcon} />
              <div className={styles.listText}>
                <PathText path={document.path} />
              </div>
              {missing.has(document.path) && <MissingBadge />}
              <PageInput
                page={document.page}
                label={`Page for ${pathLeaf(document.path)}`}
                onChange={(page) => onChange({ ...state, documents: replaceAt(state.documents, index, { ...document, page }) })}
              />
              <IconButton
                icon={X}
                size="sm"
                aria-label={`Remove ${pathLeaf(document.path)}`}
                className={styles.removeButton}
                onClick={() => onChange({ ...state, documents: removeAt(state.documents, index) })}
              />
            </li>
          ))}
        </ul>
      )}
      <div className={styles.actions}>
        <Button size="sm" variant="secondary" icon={FilePlus} onClick={() => void add()}>
          Add documents…
        </Button>
      </div>
    </div>
  );
}

function PageInput({ page, label, onChange }: { page: number | null; label: string; onChange: (page: number | null) => void }) {
  const [text, setText] = useState(page === null ? "" : String(page));
  const [invalid, setInvalid] = useState(false);
  const [lastPage, setLastPage] = useState(page);
  // Follow outside changes (e.g. discard).
  if (page !== lastPage) {
    setLastPage(page);
    setText(page === null ? "" : String(page));
    setInvalid(false);
  }
  return (
    <span className={styles.pageField}>
      <span className={styles.pageLabel} aria-hidden="true">
        Page
      </span>
      <Input
        size="sm"
        inputMode="numeric"
        value={text}
        placeholder="–"
        aria-label={label}
        aria-invalid={invalid || undefined}
        className={styles.pageInput}
        onChange={(event) => {
          setText(event.target.value);
          const parsed = parsePage(event.target.value);
          setInvalid(!parsed.ok);
          if (parsed.ok && parsed.value !== page) {
            setLastPage(parsed.value);
            onChange(parsed.value);
          }
        }}
        onBlur={() => {
          setText(page === null ? "" : String(page));
          setInvalid(false);
        }}
      />
    </span>
  );
}

// ---- Browser -----------------------------------------------------------------------------------

function BrowserEditor({ state, onChange }: { state: StateOf<"browser">; onChange: (state: AppState) => void }) {
  const errorId = useId();
  const [address, setAddress] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [target, setTarget] = useState(Math.max(0, state.windows.length - 1));
  const windowIndex = Math.min(target, Math.max(0, state.windows.length - 1));
  const tabCount = state.windows.reduce((sum, w) => sum + w.tabs.length, 0);

  const add = (event: FormEvent) => {
    event.preventDefault();
    const parsed = parseTabAddress(address);
    if (!parsed.ok) {
      setError(parsed.error);
      return;
    }
    onChange({ ...state, windows: addBrowserTab(state.windows, windowIndex, parsed.value) });
    setAddress("");
    setError(null);
  };

  return (
    <div className={styles.panel}>
      {state.windows.length === 0 ? (
        <div className={styles.empty}>No tabs saved. Add a web address below.</div>
      ) : (
        <div>
          {state.windows.map((window, wi) => (
            <div key={wi} className={styles.windowGroup}>
              {(state.windows.length > 1 || state.profile) && (
                <div className={styles.windowLabel}>
                  {state.windows.length > 1 ? `Window ${wi + 1} · ${pluralize(window.tabs.length, "tab")}` : pluralize(tabCount, "tab")}
                  {wi === 0 && state.profile && (
                    <span className={styles.groupMeta}>
                      · Profile <span className="mono">{state.profile}</span>
                    </span>
                  )}
                </div>
              )}
              <ul className={styles.list} aria-label={state.windows.length > 1 ? `Window ${wi + 1} tabs` : "Tabs"}>
                {window.tabs.map((tab, ti) => (
                  <li key={`${tab.url}-${ti}`} className={styles.listRow}>
                    <Icon icon={tab.pinned ? Pin : Globe} size={16} className={styles.listIcon} />
                    <div className={styles.listText}>
                      <span className={styles.listTitle}>
                        {tab.title || tab.url}
                        {window.activeTab === ti && <span className="sr-only"> (selected tab)</span>}
                      </span>
                      <span className={cx(styles.listMeta, "mono selectable")} title={tab.url}>
                        {tab.url}
                      </span>
                    </div>
                    {window.activeTab === ti && <span className={styles.selectedDot} title="Selected tab" aria-hidden="true" />}
                    <IconButton
                      icon={X}
                      size="sm"
                      aria-label={`Remove tab ${tab.title || tab.url}`}
                      className={styles.removeButton}
                      onClick={() => onChange({ ...state, windows: removeBrowserTab(state.windows, wi, ti) })}
                    />
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      )}

      <form className={styles.addRow} onSubmit={add} noValidate>
        <div>
          <Input
            mono
            className={styles.sansPlaceholder}
            value={address}
            placeholder="Add a web address, like example.com/docs"
            aria-label="Web address to add as a tab"
            aria-invalid={error ? true : undefined}
            aria-describedby={error ? errorId : undefined}
            onChange={(event) => {
              setAddress(event.target.value);
              if (error) setError(null);
            }}
          />
          {error && (
            <p id={errorId} className={styles.fieldError}>
              {error}
            </p>
          )}
        </div>
        {state.windows.length > 1 && (
          <Select
            value={String(windowIndex)}
            onChange={(value) => setTarget(Number(value))}
            options={state.windows.map((_, i) => ({ value: String(i), label: `Window ${i + 1}` }))}
            aria-label="Window to add the tab to"
            className={styles.windowSelect}
          />
        )}
        <Button type="submit" variant="secondary" icon={Plus}>
          Add tab
        </Button>
      </form>
    </div>
  );
}

// ---- Terminal ----------------------------------------------------------------------------------

function TerminalEditor({
  state,
  missing,
  appName,
  onChange,
}: {
  state: StateOf<"terminal">;
  missing: ReadonlySet<string>;
  appName: string;
  onChange: (state: AppState) => void;
}) {
  const update = (index: number, patch: Partial<TerminalSession>) =>
    onChange({ ...state, sessions: replaceAt(state.sessions, index, updateTerminalSession(state.sessions[index], patch)) });
  return (
    <div className={styles.panel}>
      {state.sessions.length === 0 ? (
        <div className={styles.empty}>No sessions saved — {appName} opens with its usual window.</div>
      ) : (
        state.sessions.map((session, index) => (
          <TerminalSessionEditor
            key={index}
            index={index}
            session={session}
            missingFolder={session.workingDirectory !== null && missing.has(session.workingDirectory)}
            onChange={(patch) => update(index, patch)}
            onRemove={() => onChange({ ...state, sessions: removeAt(state.sessions, index) })}
          />
        ))
      )}
      <div className={styles.actions}>
        <Button
          size="sm"
          variant="secondary"
          icon={Plus}
          onClick={() => onChange({ ...state, sessions: [...state.sessions, emptyTerminalSession()] })}
        >
          Add session
        </Button>
      </div>
    </div>
  );
}

function TerminalSessionEditor({
  session,
  index,
  missingFolder,
  onChange,
  onRemove,
}: {
  session: TerminalSession;
  index: number;
  missingFolder: boolean;
  onChange: (patch: Partial<TerminalSession>) => void;
  onRemove: () => void;
}) {
  const command = session.startupCommand ?? "";
  const hasCommand = command.trim() !== "";
  return (
    <section className={styles.session} aria-label={`Session ${index + 1}`}>
      <div className={styles.sessionHeader}>
        <h4 className={styles.sessionTitle}>
          Session {index + 1}
          {session.windowTitle && <span className={styles.sessionMeta}>{session.windowTitle}</span>}
        </h4>
        <IconButton icon={Trash} size="sm" aria-label={`Remove session ${index + 1}`} tooltip onClick={onRemove} />
      </div>
      <div className={styles.twoColumns}>
        <Field label="Working folder" optional error={missingFolder ? "This folder no longer exists." : undefined}>
          {(control) => (
            <PathPicker
              id={control.id}
              aria-describedby={control["aria-describedby"]}
              kind="folder"
              value={session.workingDirectory}
              invalid={missingFolder}
              placeholder="Home folder"
              pickerTitle="Choose the session's working folder"
              onChange={(path) => onChange({ workingDirectory: path })}
            />
          )}
        </Field>
        <Field label="Profile" optional>
          {(control) => (
            <Input
              {...control}
              value={session.profile ?? ""}
              placeholder="Default profile"
              onChange={(event) => onChange({ profile: event.target.value === "" ? null : event.target.value })}
            />
          )}
        </Field>
      </div>
      <Field label="Startup command" optional hint="Runs in this session after it opens — only once you allow it below.">
        {(control) => (
          <Input
            {...control}
            mono
            value={command}
            placeholder="e.g. npm run dev"
            onChange={(event) => onChange({ startupCommand: event.target.value === "" ? null : event.target.value })}
          />
        )}
      </Field>
      {hasCommand && (
        <div className={styles.approval}>
          <Switch
            checked={session.startupCommandApproved}
            onChange={(approved) => onChange({ startupCommandApproved: approved })}
            label="Run this command when restoring"
            description="Reframe runs it exactly as written every time this workspace is restored. Only turn this on for a command you wrote yourself or fully trust."
          />
          {!session.startupCommandApproved && (
            <p className={cx(styles.callout, styles.calloutWarning)}>
              <Icon icon={ShieldAlert} size={14} />
              <span className={styles.calloutText}>
                Not allowed yet, so it won&rsquo;t run. Commands in imported workspaces always start out this way, and editing a
                command turns this off again.
              </span>
            </p>
          )}
        </div>
      )}
    </section>
  );
}

// ---- Media -------------------------------------------------------------------------------------

function MediaEditor({ state, onChange }: { state: StateOf<"media">; onChange: (state: AppState) => void }) {
  return (
    <div className={styles.panel}>
      <Field
        label="Playlist, album or station"
        optional
        hint="Paste the link or URI from the app's Share menu, e.g. spotify:playlist:37i9dQZF1DXcBWIGoYBM5M."
      >
        {(control) => (
          <Input
            {...control}
            mono
            value={state.uri ?? ""}
            placeholder="spotify:playlist:…"
            onChange={(event) => onChange({ ...state, uri: event.target.value === "" ? null : event.target.value })}
          />
        )}
      </Field>
      <Field label="Name" optional hint="Shown in Reframe instead of the link.">
        {(control) => (
          <Input
            {...control}
            value={state.label ?? ""}
            placeholder="e.g. Focus mix"
            onChange={(event) => onChange({ ...state, label: event.target.value === "" ? null : event.target.value })}
          />
        )}
      </Field>
    </div>
  );
}
