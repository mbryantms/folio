"use client";

import * as React from "react";
import { Check, X } from "lucide-react";

import { cn } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";

import type { OptionsEndpoint } from "../field-registry";
import { useFilterOptions } from "../use-filter-options";

export type MultiSelectEditorProps = {
  value: unknown;
  onChange: (value: string[]) => void;
  /** Closed list of options (used by enums). */
  staticOptions?: readonly string[];
  /** Async lookup endpoint (used by genres/tags/credits). */
  endpoint?: OptionsEndpoint;
  /** Optional library scope passed to the options endpoint. */
  library?: string;
  placeholder?: string;
};

/** Shared multi-select pill input used by enum (`in`/`not_in`) and
 *  multi (`includes_*`/`excludes`) editors. Supports both static option
 *  lists (closed enums) and async-fetched ones (junction tables). */
export function MultiSelectEditor({
  value,
  onChange,
  staticOptions,
  endpoint,
  library,
  placeholder = "Add value…",
}: MultiSelectEditorProps) {
  const selected = Array.isArray(value) ? (value as string[]) : [];
  const [open, setOpen] = React.useState(false);
  const [search, setSearch] = React.useState("");

  const remote = useFilterOptions(endpoint, { library, q: search });
  const options = staticOptions
    ? Array.from(staticOptions)
    : (remote.data?.values ?? []);

  function toggle(v: string) {
    if (selected.includes(v)) {
      onChange(selected.filter((x) => x !== v));
    } else {
      onChange([...selected, v]);
    }
  }

  return (
    <div className="flex w-full flex-col gap-1">
      {/* See web/components/ui/combobox.tsx — the popover relies on
          `<PopoverPortalContainer>` from the wrapping dialog (when
          there is one) to render inside the dialog tree. */}
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button
            type="button"
            variant="outline"
            role="combobox"
            aria-expanded={open}
            className={cn(
              "min-h-9 w-full justify-start text-left font-normal",
              selected.length === 0 && "text-muted-foreground",
            )}
          >
            <span className="truncate">
              {selected.length === 0
                ? placeholder
                : `${selected.length} selected`}
            </span>
          </Button>
        </PopoverTrigger>
        <PopoverContent
          className="w-[var(--radix-popover-trigger-width)] p-0"
          align="start"
        >
          <Command shouldFilter={!!staticOptions}>
            <CommandInput
              placeholder="Search…"
              value={search}
              onValueChange={setSearch}
            />
            <CommandList>
              <CommandEmpty>
                {endpoint && remote.isLoading ? "Loading…" : "No results."}
              </CommandEmpty>
              <CommandGroup>
                {options.map((opt) => {
                  const checked = selected.includes(opt);
                  return (
                    <CommandItem
                      key={opt}
                      value={opt}
                      onSelect={() => toggle(opt)}
                    >
                      <Check
                        className={cn(
                          "mr-2 h-4 w-4",
                          checked ? "opacity-100" : "opacity-0",
                        )}
                      />
                      {opt}
                    </CommandItem>
                  );
                })}
              </CommandGroup>
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
      {selected.length > 0 ? (
        <div className="flex flex-wrap gap-1">
          {selected.map((v) => (
            <Badge key={v} variant="secondary" className="gap-1 pr-1">
              {v}
              <button
                type="button"
                onClick={() => onChange(selected.filter((x) => x !== v))}
                className="hover:bg-muted-foreground/20 rounded-sm"
                aria-label={`Remove ${v}`}
              >
                <X className="h-3 w-3" />
              </button>
            </Badge>
          ))}
        </div>
      ) : null}
    </div>
  );
}
