import { useNow } from "@/hooks/useNow";
import { cx } from "@/lib/cx";
import { formatAbsoluteTime, formatRelativeTime, toDate, type DateInput } from "@/lib/format";

export interface RelativeTimeProps {
  value: DateInput;
  /** Text before the time ("Restored "). */
  prefix?: string;
  /** Shown when the value is missing or invalid. */
  fallback?: string;
  className?: string;
}

/** "2 min ago" that keeps itself current; the absolute time is in the tooltip/title. */
export function RelativeTime({ value, prefix = "", fallback = "", className }: RelativeTimeProps) {
  const now = useNow();
  const date = toDate(value);
  if (!date) return fallback ? <span className={className}>{fallback}</span> : null;
  return (
    <time dateTime={date.toISOString()} title={formatAbsoluteTime(date)} className={cx("tabular", className)}>
      {prefix}
      {formatRelativeTime(date, new Date(now))}
    </time>
  );
}
