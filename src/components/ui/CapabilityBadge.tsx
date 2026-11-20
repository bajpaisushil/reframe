import { Ban, CircleCheck, CircleDot, Play } from "lucide-react";
import type { CapabilityFeature, CapabilityLevel } from "@/types";
import { Badge, type BadgeTone } from "./Badge";
import type { LucideIcon } from "./Icon";
import { Tooltip } from "./Tooltip";
import styles from "./CapabilityBadge.module.css";

interface CapabilityCopy {
  label: string;
  tone: BadgeTone;
  icon: LucideIcon;
  explanation: string;
}

/** Plain-language meaning of each capability level (also used by other views). */
export const CAPABILITY_COPY: Record<CapabilityLevel, CapabilityCopy> = {
  full: {
    label: "Full restore",
    tone: "success",
    icon: CircleCheck,
    explanation: "Reframe reopens this app with what was inside it — projects, tabs or documents — and puts its windows back.",
  },
  partial: {
    label: "Partial",
    tone: "accent",
    icon: CircleDot,
    explanation: "Reframe reopens this app and some of what was inside it. A few details may need a quick touch-up.",
  },
  launchOnly: {
    label: "Launch only",
    tone: "neutral",
    icon: Play,
    explanation: "Reframe starts this app and arranges its windows, but can't reopen what was inside it.",
  },
  unsupported: {
    label: "Not supported",
    tone: "warning",
    icon: Ban,
    explanation: "Reframe can't start this app on this computer, so it will be skipped when restoring.",
  },
};

export interface CapabilityBadgeProps {
  level: CapabilityLevel;
  /** Adapter features, listed in the tooltip when given. */
  features?: readonly CapabilityFeature[];
  /** Make the badge keyboard-focusable so the explanation is reachable without a mouse. */
  focusable?: boolean;
  className?: string;
}

/** "Full restore" / "Partial" / "Launch only" / "Not supported", with an explaining tooltip. */
export function CapabilityBadge({ level, features = [], focusable = false, className }: CapabilityBadgeProps) {
  const copy = CAPABILITY_COPY[level] ?? CAPABILITY_COPY.launchOnly;
  const content = (
    <span className={styles.tooltip}>
      <span>{copy.explanation}</span>
      {features.length > 0 && (
        <span className={styles.features}>
          {features.map((feature) => (
            <span key={feature.name} className={styles.feature} data-supported={feature.supported || undefined}>
              <span aria-hidden className={styles.mark}>
                {feature.supported ? "✓" : "–"}
              </span>
              {feature.name}
              {feature.note ? <span className={styles.note}> — {feature.note}</span> : null}
            </span>
          ))}
        </span>
      )}
    </span>
  );
  return (
    <Tooltip content={content} placement="top">
      <Badge tone={copy.tone} icon={copy.icon} className={className} tabIndex={focusable ? 0 : undefined}>
        {copy.label}
        <span className="sr-only">: {copy.explanation}</span>
      </Badge>
    </Tooltip>
  );
}
