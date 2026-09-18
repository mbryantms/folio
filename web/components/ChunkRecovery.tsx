"use client";
import { Button } from "@/components/ui/button";
import { toast } from "sonner";
/** User-initiated, once per URL per session. Never automatically discards a
 * reader/editor state, and a broken deployment cannot cause a reload loop. */
export function ChunkRecovery({ error }: { error: Error }) {
  if (
    !/ChunkLoadError|Loading chunk|Failed to fetch dynamically imported module/i.test(
      error.name + " " + error.message,
    )
  )
    return null;
  const reload = () => {
    const key = `folio:chunk-recovery:${location.pathname}`;
    try {
      if (sessionStorage.getItem(key)) {
        toast.error(
          "Reload did not repair the app. Try again after your server has updated.",
        );
        return;
      }
      sessionStorage.setItem(key, "1");
    } catch {
      /* User initiated reload remains available without storage. */
    }
    window.dispatchEvent(new Event("folio:before-reload"));
    location.reload();
  };
  return <Button onClick={reload}>Reload updated app</Button>;
}
