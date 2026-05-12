"use client";

import { Input } from "@/components/ui/input";

export type TextEditorProps = {
  value: unknown;
  onChange: (value: string) => void;
  placeholder?: string;
};

export function TextEditor({ value, onChange, placeholder }: TextEditorProps) {
  return (
    <Input
      type="text"
      value={typeof value === "string" ? value : ""}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder ?? "Value"}
    />
  );
}
