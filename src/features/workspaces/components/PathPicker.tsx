import { useState } from "react";
import { X } from "lucide-react";
import { Button, IconButton, useToast } from "@/components/ui";
import { errorMessage } from "@/lib/api";
import { cx } from "@/lib/cx";
import { parentFolder, pickFile, pickFolder } from "./pickers";
import styles from "./PathPicker.module.css";

export interface PathPickerProps {
  value: string | null;
  onChange: (path: string | null) => void;
  kind: "folder" | "file";
  /** Title of the native picker window. */
  pickerTitle: string;
  /** Shown when there is no value. */
  placeholder?: string;
  /** Wiring from a Field (the Choose… button receives the id). */
  id?: string;
  "aria-describedby"?: string;
  /** Allow clearing the value. */
  clearable?: boolean;
  invalid?: boolean;
}

/** A chosen path (Geist Mono, selectable) with "Choose…" and an optional clear button. */
export function PathPicker({
  value,
  onChange,
  kind,
  pickerTitle,
  placeholder = "Not set",
  id,
  clearable = true,
  invalid = false,
  ...rest
}: PathPickerProps) {
  const toast = useToast();
  const [picking, setPicking] = useState(false);

  const choose = async () => {
    setPicking(true);
    try {
      const start = kind === "folder" ? value : parentFolder(value);
      const path = kind === "folder" ? await pickFolder(pickerTitle, start) : await pickFile(pickerTitle, start);
      if (path) onChange(path);
    } catch (error) {
      toast.error(kind === "folder" ? "Couldn't open the folder picker" : "Couldn't open the file picker", errorMessage(error));
    } finally {
      setPicking(false);
    }
  };

  return (
    <div className={styles.row}>
      <span className={cx(styles.value, invalid && styles.invalid)} title={value ?? undefined}>
        {value ? <span className={cx(styles.path, "mono selectable")}>{value}</span> : <span className={styles.empty}>{placeholder}</span>}
        {clearable && value && (
          <IconButton icon={X} size="sm" aria-label="Clear" className={styles.clear} onClick={() => onChange(null)} />
        )}
      </span>
      <Button id={id} variant="secondary" loading={picking} onClick={() => void choose()} aria-describedby={rest["aria-describedby"]}>
        Choose…
      </Button>
    </div>
  );
}
