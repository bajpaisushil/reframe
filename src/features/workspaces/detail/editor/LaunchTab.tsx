import { useEffect, useRef, useState } from "react";
import { Plus, TriangleAlert, X } from "lucide-react";
import { Button, Icon, IconButton, Input, useToast } from "@/components/ui";
import { api, errorMessage } from "@/lib/api";
import { cx } from "@/lib/cx";
import type { ApplicationEntry, EntryHealth } from "@/types";
import { PathPicker } from "../../components/PathPicker";
import { parentFolder, pickFile } from "../../components/pickers";
import { launchTargetText, packageKindLabel, removeAt, replaceAt, withLaunchTarget } from "../../model/entries";
import styles from "./editor.module.css";

export interface LaunchTabProps {
  entry: ApplicationEntry;
  health: EntryHealth | undefined;
  onUpdate: (update: (entry: ApplicationEntry) => ApplicationEntry) => void;
}

const prose = (text: string) => text.replace(/ - /g, " — ");

/** How the app is started on this computer: program, extra arguments, working folder. */
export function LaunchTab({ entry, health, onUpdate }: LaunchTabProps) {
  const toast = useToast();
  const [changing, setChanging] = useState(false);
  const target = launchTargetText(entry.launch);
  const workingDirectory = entry.launch.workingDirectory;
  const folderMissing = workingDirectory !== null && (health?.missingPaths.includes(workingDirectory) ?? false);

  const changeProgram = async () => {
    let path: string | null;
    try {
      path = await pickFile(`Choose the program for ${entry.displayName}`, parentFolder(entry.launch.executable));
    } catch (error) {
      toast.error("Couldn't open the file picker", errorMessage(error));
      return;
    }
    if (!path) return;
    setChanging(true);
    try {
      // Validates the file and resolves app bundles / packages the same way as adding an app.
      const resolved = await api.createManualEntry({ kind: "executable", path });
      onUpdate((current) => withLaunchTarget(current, resolved.launch));
    } catch (error) {
      toast.error("Reframe can't start that file", errorMessage(error));
    } finally {
      setChanging(false);
    }
  };

  return (
    <div className={styles.panel}>
      <div className={styles.group}>
        <div className={styles.groupHeader}>
          <h4 className={styles.groupTitle}>Program</h4>
          {entry.launch.package && <span className={styles.groupMeta}>{packageKindLabel(entry.launch.package.kind)}</span>}
        </div>
        <div className={styles.valueRow}>
          <span className={cx(styles.valueBox, target && "mono selectable")} title={target ?? undefined}>
            {target ? <span className={styles.valueText}>{target}</span> : <span className={styles.valueEmpty}>Not set — choose the app&rsquo;s program</span>}
          </span>
          <Button variant="secondary" loading={changing} onClick={() => void changeProgram()}>
            Change…
          </Button>
        </div>
        {health && !health.available && health.issues[0] && (
          <p className={cx(styles.callout, styles.calloutWarning)}>
            <Icon icon={TriangleAlert} size={14} />
            <span className={styles.calloutText}>{prose(health.issues[0])}</span>
          </p>
        )}
      </div>

      <div className={styles.group}>
        <h4 className={styles.groupTitle}>Extra arguments</h4>
        <p className={styles.hint}>Passed to the app exactly as written, one per field, after anything Reframe adds itself.</p>
        <ArgumentsEditor
          values={entry.launch.arguments}
          onChange={(args) => onUpdate((current) => ({ ...current, launch: { ...current.launch, arguments: args } }))}
        />
      </div>

      <div className={styles.group}>
        <h4 className={styles.groupTitle}>Working folder</h4>
        <p className={styles.hint}>The folder the app starts in. Leave it empty to use the app&rsquo;s own default.</p>
        <PathPicker
          kind="folder"
          value={workingDirectory}
          invalid={folderMissing}
          placeholder="App default"
          pickerTitle={`Choose where ${entry.displayName} starts`}
          onChange={(path) => onUpdate((current) => ({ ...current, launch: { ...current.launch, workingDirectory: path } }))}
        />
        {folderMissing && <p className={styles.fieldError}>This folder no longer exists.</p>}
      </div>
    </div>
  );
}

function ArgumentsEditor({ values, onChange }: { values: readonly string[]; onChange: (values: string[]) => void }) {
  const refs = useRef<Array<HTMLInputElement | null>>([]);
  const [focusIndex, setFocusIndex] = useState<number | null>(null);

  useEffect(() => {
    if (focusIndex === null) return;
    refs.current[focusIndex]?.focus();
    setFocusIndex(null);
  }, [focusIndex, values.length]);

  return (
    <div className={styles.group}>
      {values.length > 0 && (
        <ul className={styles.argList}>
          {values.map((value, index) => (
            <li key={index} className={styles.argRow}>
              <Input
                ref={(el) => {
                  refs.current[index] = el;
                }}
                mono
                size="sm"
                value={value}
                placeholder="--argument"
                aria-label={`Argument ${index + 1}`}
                onChange={(event) => onChange(replaceAt(values, index, event.target.value))}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    onChange([...values.slice(0, index + 1), "", ...values.slice(index + 1)]);
                    setFocusIndex(index + 1);
                  } else if (event.key === "Backspace" && value === "") {
                    event.preventDefault();
                    onChange(removeAt(values, index));
                    setFocusIndex(Math.max(0, index - 1));
                  }
                }}
              />
              <IconButton icon={X} size="sm" aria-label={`Remove argument ${index + 1}`} onClick={() => onChange(removeAt(values, index))} />
            </li>
          ))}
        </ul>
      )}
      <div className={styles.actions}>
        <Button
          size="sm"
          variant="secondary"
          icon={Plus}
          onClick={() => {
            onChange([...values, ""]);
            setFocusIndex(values.length);
          }}
        >
          Add argument
        </Button>
      </div>
    </div>
  );
}
