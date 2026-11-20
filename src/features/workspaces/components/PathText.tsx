import { cx } from "@/lib/cx";
import { pathLeaf } from "../model/entries";
import styles from "./PathText.module.css";

export interface PathTextProps {
  path: string;
  /** `split` shows the folder or file name prominently with its location after it. */
  variant?: "split" | "mono";
  className?: string;
}

/** A file-system path that stays readable when long (full path in the tooltip, selectable). */
export function PathText({ path, variant = "split", className }: PathTextProps) {
  if (variant === "mono") {
    return (
      <span className={cx(styles.mono, "mono selectable", className)} title={path}>
        {path}
      </span>
    );
  }
  const leaf = pathLeaf(path);
  const location = path.slice(0, Math.max(0, path.length - leaf.length)).replace(/[\\/]+$/, "");
  return (
    <span className={cx(styles.split, className)} title={path}>
      <span className={cx(styles.leaf, "selectable")}>{leaf}</span>
      {location && <span className={cx(styles.location, "mono selectable")}>{location}</span>}
    </span>
  );
}
