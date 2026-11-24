"use client";

/**
 * Linked reading lists (PR 4): the events Up Next offers beside the main
 * line ("Start House of M · 0/64"). Starting one reads its first unread
 * issue in the event's own list, remembering the list it came from
 * (`?via=`) so the event's end returns there.
 */
import * as React from "react";
import { useRouter } from "next/navigation";
import { Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import type { NextUpDetour } from "@/lib/api/types";
import { displayListName, ensureListView } from "@/lib/reader/list-context";
import { noteReplace } from "@/lib/ui/back-flight";
import { readerUrl } from "@/lib/urls";

type Props = {
  detours: readonly NextUpDetour[] | undefined;
  /** The `?cbl=` view being read (the list to come back to). */
  viaViewId: string | null;
  /** Runs before navigating (closes the card). */
  onStart?: () => void;
};

export function DetourButtons(props: Props) {
  if (!props.detours || props.detours.length === 0) return null;
  return <DetourList {...props} detours={props.detours} />;
}

function DetourList({
  detours,
  viaViewId,
  onStart,
}: Props & { detours: readonly NextUpDetour[] }) {
  const router = useRouter();
  const [pending, setPending] = React.useState<string | null>(null);
  const start = async (d: NextUpDetour) => {
    setPending(d.link_id);
    const view = await ensureListView(d.list_id, d.view_id);
    const href = readerUrl(
      d.first_unread.series_slug,
      d.first_unread.issue_slug,
      {
        cbl: view,
        via: view ? viaViewId : null,
      },
    );
    onStart?.();
    noteReplace(href);
    router.replace(href);
  };
  return (
    <div className="flex flex-col gap-2 pt-1">
      <p className="text-muted-foreground text-xs">
        {detours.length === 1
          ? "Or read the event here first"
          : "Or read an event here first"}
      </p>
      {detours.map((d) => (
        <Button
          key={d.link_id}
          variant="outline"
          className="h-auto w-full justify-between gap-3 py-2"
          disabled={pending != null}
          onClick={() => void start(d)}
        >
          <span className="min-w-0 truncate text-left">
            Start {displayListName(d.name)}
          </span>
          <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
            {pending === d.link_id ? (
              <Loader2 className="size-3 animate-spin" aria-hidden />
            ) : (
              `${d.read}/${d.in_library}`
            )}
          </span>
        </Button>
      ))}
    </div>
  );
}
