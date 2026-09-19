import { useEffect, useRef, useState } from "react";

/**
 * Number input that lets the rider type freely (including a temporarily empty
 * field) and only propagates finite values. The draft text is local state, so
 * hosts must key rows with stable ids rather than indexes or a deleted row's
 * draft can be reused by its neighbour.
 */
export function EditableNumberInput({
  value,
  onValueChange,
  min,
  max,
  step,
  disabled,
  "aria-label": ariaLabel,
}: {
  value: number;
  onValueChange: (value: number) => void;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  "aria-label"?: string;
}) {
  const [text, setText] = useState(String(value));
  const focused = useRef(false);

  useEffect(() => {
    if (!focused.current) setText(String(value));
  }, [value]);

  const commit = () => {
    focused.current = false;
    const parsed = Number(text);
    if (text.trim() === "" || !Number.isFinite(parsed)) {
      setText(String(value));
      return;
    }
    onValueChange(parsed);
    setText(String(parsed));
  };

  return (
    <input
      type="number"
      min={min}
      max={max}
      step={step}
      disabled={disabled}
      aria-label={ariaLabel}
      value={text}
      onFocus={() => { focused.current = true; }}
      onChange={(event) => {
        const next = event.target.value;
        setText(next);
        if (next.trim() === "") return;
        const parsed = Number(next);
        if (Number.isFinite(parsed)) onValueChange(parsed);
      }}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") event.currentTarget.blur();
      }}
    />
  );
}
