"use client";

import * as React from "react";
import { useRouter } from "next/navigation";
import { Sparkles } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { jsonFetch } from "@/lib/api/queries";
import type { SeriesListView } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import { seriesUrl } from "@/lib/urls";

/**
 * "Surprise me" — jumps to a random series detail page (audit 3.7
 * discovery). Backed by the server's `sort=random` (`ORDER BY random()`),
 * so the pick is unbiased across the whole accessible library rather than
 * a client-side sample. Lives in the Recently Added rail header on Home.
 */
export function SurpriseMeButton({ className }: { className?: string }) {
  const router = useRouter();
  const [busy, setBusy] = React.useState(false);

  async function surpriseMe() {
    if (busy) return;
    setBusy(true);
    try {
      const res = await jsonFetch<SeriesListView>(
        "/series?sort=random&limit=1",
      );
      const pick = res.items[0];
      if (pick) router.push(seriesUrl(pick));
      else toast.info("No series to surprise you with yet.");
    } catch {
      toast.error("Couldn't pick a series — try again.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <Button
      type="button"
      variant="ghost"
      size="sm"
      onClick={surpriseMe}
      disabled={busy}
      className={cn("text-muted-foreground hover:text-foreground", className)}
    >
      <Sparkles aria-hidden="true" className="mr-1 size-3.5" />
      Surprise me
    </Button>
  );
}
