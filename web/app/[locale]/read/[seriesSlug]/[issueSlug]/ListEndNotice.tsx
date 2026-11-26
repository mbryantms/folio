"use client";

/**
 * Linked reading lists: the end of an event when the list it is part of
 * has nothing left to read either (nor the parts after it). The reading
 * order ends here, so the card says so and offers the way back to that
 * list instead of the generic "caught up" state.
 */
import * as React from "react";
import { useRouter } from "next/navigation";

import { Button } from "@/components/ui/button";
import type { NextUpView } from "@/lib/api/types";
import { displayListName, ensureListView } from "@/lib/reader/list-context";
import { toast } from "sonner";

/** The list to go back to, when Up Next ended with one and no issue. */
export function listEndTarget(
  data: NextUpView | undefined,
): NonNullable<NextUpView["switch_to"]> | null {
  const sw = data?.switch_to;
  if (!data || data.target || !sw || sw.collection || !sw.list_id) return null;
  return sw;
}

export function ListEndNotice({
  target,
  onNavigate,
}: {
  target: NonNullable<NextUpView["switch_to"]>;
  /** Runs before navigating (closes the card). */
  onNavigate?: () => void;
}) {
  const router = useRouter();
  const [busy, setBusy] = React.useState(false);
  const name = displayListName(target.name);
  const back = async () => {
    if (!target.list_id) return;
    setBusy(true);
    try {
      const view = await ensureListView(target.list_id, target.view_id);
      if (!view) {
        toast.error(`Couldn't open ${name}. Try again.`);
        return;
      }
      onNavigate?.();
      router.push(`/views/${view}`);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div data-testid="list-end-notice">
      <p className="text-base leading-snug font-semibold">
        That&apos;s the end of this event.
      </p>
      <p className="text-muted-foreground mt-1 text-sm">
        Nothing left to read in {name}.
      </p>
      <Button
        className="mt-3 w-full"
        disabled={busy}
        onClick={() => void back()}
      >
        Back to {name}
      </Button>
    </div>
  );
}
