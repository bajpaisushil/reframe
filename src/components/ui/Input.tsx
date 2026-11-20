import type { InputHTMLAttributes, ReactNode, Ref, SelectHTMLAttributes, TextareaHTMLAttributes } from "react";
import { ChevronDown } from "lucide-react";
import { cx } from "@/lib/cx";
import { Icon, type LucideIcon } from "./Icon";
import styles from "./Input.module.css";

export interface InputProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "size"> {
  size?: "sm" | "md";
  /** Icon inside the field on the left (e.g. search). */
  icon?: LucideIcon;
  /** Technical input (paths, commands) in Geist Mono. */
  mono?: boolean;
  ref?: Ref<HTMLInputElement>;
}

export function Input({ size = "md", icon, mono = false, className, type = "text", ref, ...rest }: InputProps) {
  const input = (
    <input
      ref={ref}
      type={type}
      className={cx(styles.control, styles.input, styles[size], icon && styles.withIcon, mono && styles.mono, className)}
      spellCheck={mono ? false : rest.spellCheck}
      autoComplete={rest.autoComplete ?? "off"}
      {...rest}
    />
  );
  if (!icon) return input;
  return (
    <span className={styles.iconWrap}>
      <Icon icon={icon} size={16} className={styles.leadingIcon} />
      {input}
    </span>
  );
}

export interface TextareaProps extends TextareaHTMLAttributes<HTMLTextAreaElement> {
  mono?: boolean;
  ref?: Ref<HTMLTextAreaElement>;
}

export function Textarea({ mono = false, className, rows = 3, ref, ...rest }: TextareaProps) {
  return (
    <textarea
      ref={ref}
      rows={rows}
      className={cx(styles.control, styles.textarea, mono && styles.mono, className)}
      {...rest}
    />
  );
}

export interface SelectOption<T extends string = string> {
  value: T;
  label: string;
  disabled?: boolean;
}

export interface SelectProps<T extends string = string>
  extends Omit<SelectHTMLAttributes<HTMLSelectElement>, "onChange" | "value" | "size"> {
  value: T;
  onChange: (value: T) => void;
  options: readonly SelectOption<T>[];
  size?: "sm" | "md";
  placeholder?: ReactNode;
  ref?: Ref<HTMLSelectElement>;
}

/** A styled native select (native menus are the most accessible choice on every platform). */
export function Select<T extends string = string>({
  value,
  onChange,
  options,
  size = "md",
  className,
  ref,
  ...rest
}: SelectProps<T>) {
  return (
    <span className={cx(styles.selectWrap, className)}>
      <select
        ref={ref}
        value={value}
        onChange={(event) => onChange(event.target.value as T)}
        className={cx(styles.control, styles.select, styles[size])}
        {...rest}
      >
        {options.map((option) => (
          <option key={option.value} value={option.value} disabled={option.disabled}>
            {option.label}
          </option>
        ))}
      </select>
      <Icon icon={ChevronDown} size={14} className={styles.chevron} />
    </span>
  );
}
