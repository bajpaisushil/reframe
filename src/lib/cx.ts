/** Join class names, skipping falsy values: `cx(styles.a, active && styles.active)`. */
export function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}
