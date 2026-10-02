"use client";

/**
 * `<RelationshipKindSelect>` — the grouped "Relationship" picker (WP-7.5,
 * reworked in WP-7.7) fed by the server's kind catalogue
 * (`GET /relationship-kinds`). A searchable command palette in a popover:
 * every group (Story · Publication history · Editions & contents ·
 * Advanced) has a heading, the list is tall enough to show the groups
 * without a cramped scroll, and typing filters by label or group. While
 * the catalogue loads the trigger is a skeleton — never the raw kind key.
 *
 * Used by the series page's add / edit relationship forms and the admin
 * review page's "Edit kind".
 */

import { Check, ChevronsUpDown } from "lucide-react";
import * as React from "react";

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
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { useRelationshipKinds } from "@/lib/api/queries";
import type { RelationshipKind } from "@/lib/api/types";
import { groupedKinds, kindInfo } from "@/lib/relationships";
import { cn } from "@/lib/utils";

/** The list is as tall as the room Radix reports for the popover (minus
 *  the search row), capped at 26rem — never a fixed short box while the
 *  screen has space. */
const KIND_LIST_MAX_H =
  "max-h-[min(26rem,calc(var(--radix-popover-content-available-height,26rem)-2.75rem))]";

/** Group headings stay readable while scrolling: sticky on the popover
 *  surface. The group's default `overflow-hidden` would scope `sticky` to
 *  the group itself, so it is lifted. */
const STICKY_GROUP_HEADINGS =
  "overflow-visible [&_[cmdk-group-heading]]:bg-popover [&_[cmdk-group-heading]]:sticky [&_[cmdk-group-heading]]:top-0 [&_[cmdk-group-heading]]:z-10";

/** cmdk filter: match the label + group heading text an item carries in
 *  its `value`, case-insensitively (substring, not fuzzy). */
export function kindFilter(itemValue: string, search: string): number {
  const q = search.trim().toLowerCase();
  if (!q) return 1;
  return itemValue.toLowerCase().includes(q) ? 1 : 0;
}

export function RelationshipKindSelect({
  id,
  value,
  onChange,
  ariaLabel = "Relationship",
  disabled,
  filter,
  className,
}: {
  id?: string;
  value: RelationshipKind;
  onChange: (kind: RelationshipKind) => void;
  ariaLabel?: string;
  disabled?: boolean;
  /** Restrict the choices (e.g. arc targets take arc-capable kinds only). */
  filter?: (kind: RelationshipKind) => boolean;
  className?: string;
}) {
  const catalogue = useRelationshipKinds();
  const [open, setOpen] = React.useState(false);

  if (!catalogue.data) {
    return (
      <Skeleton
        data-testid="relationship-kind-loading"
        aria-label={`${ariaLabel} (loading)`}
        className={cn("h-9 w-full", className)}
      />
    );
  }

  const groups = groupedKinds(catalogue.data)
    .map((g) => ({
      ...g,
      kinds: filter ? g.kinds.filter((k) => filter(k.kind)) : g.kinds,
    }))
    .filter((g) => g.kinds.length > 0);
  const selected = kindInfo(catalogue.data, value);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          id={id}
          type="button"
          variant="outline"
          role="combobox"
          aria-expanded={open}
          aria-label={ariaLabel}
          disabled={disabled}
          className={cn("w-full justify-between font-normal", className)}
        >
          <span className="truncate">{selected?.label ?? "Choose…"}</span>
          <ChevronsUpDown
            aria-hidden
            className="ml-2 h-4 w-4 shrink-0 opacity-50"
          />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        // One scroller only: the popover itself never scrolls (overrides
        // the default `overflow-y-auto`); the themed ScrollArea below
        // does, sized to the space Radix reports below/above the trigger.
        className="w-[min(320px,calc(100vw-2rem))] overflow-hidden p-0"
      >
        <Command
          filter={kindFilter}
          // Open on the current kind (highlighted, scrolled into view).
          defaultValue={
            selected
              ? `${selected.label} · ${groups.find((g) => g.group === selected.group)?.label ?? ""}`
              : undefined
          }
        >
          <CommandInput placeholder="Search relationships…" />
          <ScrollArea
            type="auto"
            viewportClassName={KIND_LIST_MAX_H}
            data-testid="relationship-kind-scroll"
          >
            {/* `pr-2.5` reserves the ScrollArea's overlay scrollbar gutter
                (`w-2.5`) so sticky group headings and highlighted rows
                stop short of the scrollbar instead of painting over it. */}
            <CommandList className="max-h-none overflow-visible pr-2.5">
              <CommandEmpty>No matching relationship.</CommandEmpty>
              {groups.map((g) => (
                <CommandGroup
                  key={g.group}
                  heading={g.label}
                  className={STICKY_GROUP_HEADINGS}
                >
                  {g.kinds.map((k) => (
                    <CommandItem
                      key={k.kind}
                      value={`${k.label} · ${g.label}`}
                      onSelect={() => {
                        onChange(k.kind);
                        setOpen(false);
                      }}
                    >
                      <Check
                        aria-hidden
                        className={cn(
                          "h-4 w-4",
                          k.kind === value ? "opacity-100" : "opacity-0",
                        )}
                      />
                      {k.label}
                    </CommandItem>
                  ))}
                </CommandGroup>
              ))}
            </CommandList>
          </ScrollArea>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
