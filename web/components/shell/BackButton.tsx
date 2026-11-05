"use client";

import { ChevronLeft } from "lucide-react";
import { useRouter } from "next/navigation";

import { startBackFlight, useCanGoBack } from "@/lib/ui/back-flight";
import { cn } from "@/lib/utils";

/**
 * In-app Back for detail pages: returns to the page you came from, with
 * the header cover flying back into its card (lib/ui/back-flight.ts).
 * Renders only when this tab has an in-app page to return to; the
 * breadcrumbs beside it cover deep links and reloads.
 */
export function BackButton({ className }: { className?: string }) {
  const router = useRouter();
  const canGoBack = useCanGoBack();
  if (!canGoBack) return null;
  return (
    <>
      <button
        type="button"
        data-back-button=""
        onClick={() => {
          startBackFlight();
          router.back();
        }}
        className={cn(
          "folio-tap text-muted-foreground hover:text-foreground hover:bg-muted/60 -ml-2 inline-flex h-8 shrink-0 items-center gap-0.5 rounded-md pr-2.5 pl-1.5 text-xs font-medium transition-colors",
          className,
        )}
      >
        <ChevronLeft className="size-4" aria-hidden="true" />
        Back
      </button>
      <span aria-hidden="true" className="bg-border h-4 w-px shrink-0" />
    </>
  );
}
