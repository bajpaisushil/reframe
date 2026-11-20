// Reframe UI primitives. Import from "@/components/ui".
//
// Conventions: icons are lucide-react at 16px (18px in navigation) with stroke 1.75 via <Icon>;
// colors, spacing and motion come from src/styles/tokens.css; every component works in both themes.

export { AppIcon, appIconCache, type AppIconProps, type AppIconSize } from "./AppIcon";
export { AppIconStack, type AppIconStackApp, type AppIconStackProps } from "./AppIconStack";
export { Badge, type BadgeProps, type BadgeTone } from "./Badge";
export { Button, type ButtonProps, type ButtonSize, type ButtonVariant } from "./Button";
export { CAPABILITY_COPY, CapabilityBadge, type CapabilityBadgeProps } from "./CapabilityBadge";
export { Card, Panel, type CardProps, type PanelProps } from "./Card";
export { Checkbox, type CheckboxProps } from "./Checkbox";
export { ColorPicker, type ColorPickerProps } from "./ColorPicker";
export { ConfirmDialog, type ConfirmDialogProps } from "./ConfirmDialog";
export { Dialog, type DialogProps } from "./Dialog";
export { Divider, type DividerProps } from "./Divider";
export { EmojiPicker, type EmojiPickerProps } from "./EmojiPicker";
export { EmptyState, type EmptyStateProps } from "./EmptyState";
export { Field, type FieldControlProps, type FieldProps } from "./Field";
export { Icon, type IconProps, type LucideIcon } from "./Icon";
export { IconButton, type IconButtonProps } from "./IconButton";
export { Input, Select, Textarea, type InputProps, type SelectOption, type SelectProps, type TextareaProps } from "./Input";
export { KeyValue, KeyValueList, type KeyValueListProps, type KeyValueProps } from "./KeyValue";
export { Kbd, type KbdProps } from "./Kbd";
export { Menu, type MenuEntry, type MenuItem, type MenuProps, type MenuTriggerProps } from "./Menu";
export { Popover, type PopoverProps, type PopoverTriggerProps } from "./Popover";
export { ProgressBar, type ProgressBarProps } from "./ProgressBar";
export { RelativeTime, type RelativeTimeProps } from "./RelativeTime";
export { SectionHeader, SectionLink, type SectionHeaderProps, type SectionLinkProps } from "./SectionHeader";
export { SegmentedControl, type SegmentOption, type SegmentedControlProps } from "./SegmentedControl";
export { Skeleton, type SkeletonProps } from "./Skeleton";
export { Spinner, type SpinnerProps } from "./Spinner";
export {
  ENTRY_STATUS_COPY,
  RUN_STATUS_COPY,
  RunStatusPill,
  StatusPill,
  type RunStatusPillProps,
  type StatusPillProps,
} from "./StatusPill";
export { Switch, type SwitchProps } from "./Switch";
export { TabPanel, Tabs, type TabItem, type TabPanelProps, type TabsProps } from "./Tabs";
export { ToastProvider, useToast, type ToastApi, type ToastOptions, type ToastTone } from "./Toast";
export { Tooltip, type TooltipProps } from "./Tooltip";
export { WorkspaceIcon, type WorkspaceIconProps, type WorkspaceIconSize } from "./WorkspaceIcon";
