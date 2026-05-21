"use client";

import * as React from "react";
import { Plus } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useAddLogWidget } from "@/lib/api/mutations";
import type { LogWidgetKind, LogWidgetView } from "@/lib/api/types";

import { WIDGET_KIND_ORDER, WIDGET_REGISTRY } from "./widgets";

/** Header button + dropdown that lets the user add a widget to
 *  their grid. Filters the registry against the user's current set
 *  so each kind appears at most once — except for kinds flagged
 *  `allowMultiple` in the registry (notes), which stay selectable. */
export function AddWidgetMenu({ current }: { current: LogWidgetView[] }) {
  const add = useAddLogWidget();
  const present = React.useMemo(() => {
    const s = new Set<string>();
    for (const w of current) s.add(w.kind);
    return s;
  }, [current]);

  const items = WIDGET_KIND_ORDER.map((k) => WIDGET_REGISTRY[k]).filter(
    (def) => def.allowMultiple || !present.has(def.kind),
  );

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="outline"
          size="sm"
          disabled={items.length === 0 || add.isPending}
        >
          <Plus aria-hidden="true" className="mr-1 h-3.5 w-3.5" />
          Add widget
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-72">
        <DropdownMenuLabel>Add widget</DropdownMenuLabel>
        <DropdownMenuSeparator />
        {items.length === 0 ? (
          <p className="text-muted-foreground px-2 py-1.5 text-xs">
            Every widget is already on your grid. Remove one first, or use Reset
            to defaults.
          </p>
        ) : (
          items.map((def) => {
            const Icon = def.Icon;
            return (
              <DropdownMenuItem
                key={def.kind}
                disabled={add.isPending}
                onSelect={() =>
                  add.mutate({
                    kind: def.kind as LogWidgetKind,
                    config: def.defaultConfig,
                  })
                }
                className="items-start gap-2 py-2"
              >
                <Icon
                  aria-hidden="true"
                  className="text-muted-foreground mt-0.5 h-4 w-4 shrink-0"
                />
                <div className="flex min-w-0 flex-col">
                  <span className="text-sm font-medium">{def.displayName}</span>
                  <span className="text-muted-foreground text-xs">
                    {def.description}
                  </span>
                </div>
              </DropdownMenuItem>
            );
          })
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
