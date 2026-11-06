"use client";

import * as React from "react";

import { NativeSelect } from "@/components/folio/native-select";

/** Bordered row that holds a list's filter controls. */
export function FilterRow({ children }: { children: React.ReactNode }) {
  return (
    <div className="border-border/60 flex flex-wrap items-center gap-2 rounded-md border p-3">
      {children}
    </div>
  );
}

/** "Library: All libraries | …" select; `"all"` means unscoped. */
export function LibraryFilter({
  libraries,
  value,
  onChange,
}: {
  libraries: { id: string; name: string }[];
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <label className="flex items-center gap-1.5">
      <span className="text-muted-foreground mr-1 text-xs uppercase">
        Library
      </span>
      <NativeSelect
        size="sm"
        value={value}
        onChange={onChange}
        options={[
          { value: "all", label: "All libraries" },
          ...libraries.map((l) => ({ value: l.id, label: l.name })),
        ]}
      />
    </label>
  );
}
