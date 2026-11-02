"use client";
import { useEffect } from "react";
import { Serwist } from "@serwist/window";
import { toast } from "sonner";

/** Unregister this origin's `/sw.js` registrations; resolves to how many. */
async function unregisterOurs(): Promise<number> {
  const registrations = await navigator.serviceWorker.getRegistrations();
  const ours = registrations.filter((r) =>
    [r.active, r.waiting, r.installing].some(
      (w) => w && new URL(w.scriptURL).pathname === "/sw.js",
    ),
  );
  await Promise.all(ours.map((r) => r.unregister()));
  return ours.length;
}

/** The offline page's "Try this page again" adds `sw=bypass`, which the
 *  worker leaves to the browser. Reaching a page that way means the worker
 *  showed the offline page while the network worked: its own page loads are
 *  failing. Drop the param and say whether to reset the registration. */
function takeRecoveryParam(): boolean {
  const params = new URLSearchParams(window.location.search);
  if (params.get("sw") !== "bypass") return false;
  params.delete("sw");
  const query = params.toString();
  window.history.replaceState(
    window.history.state,
    "",
    `${window.location.pathname}${query ? `?${query}` : ""}${window.location.hash}`,
  );
  return true;
}

/** Updates never reload another open reader. Only the accepting client reloads. */
export function ServiceWorkerUpdater() {
  useEffect(() => {
    if (!("serviceWorker" in navigator)) return;
    let disposed = false;
    if (process.env.NODE_ENV !== "production") {
      // A previous production build may have controlled this dev origin.
      void unregisterOurs()
        .then((removed) => {
          if (!disposed && removed && navigator.serviceWorker.controller) {
            toast.message(
              "Development worker removed. Reload to use the development server.",
              {
                action: {
                  label: "Reload",
                  onClick: () => window.location.reload(),
                },
              },
            );
          }
        })
        .catch(() => undefined);
      return () => {
        disposed = true;
      };
    }
    const sw = new Serwist("/sw.js", { updateViaCache: "none" });
    let accepted = false;
    let activated = false;
    let pending = false;
    let notification = 0;
    let toastId: string | undefined;
    let dirty = false;
    let lastCheck = Date.now();
    const markDirty = (event: Event) => {
      if (
        (event.target as HTMLElement)?.closest(
          "form, textarea, input:not([type=search]), [contenteditable=true]",
        )
      )
        dirty = true;
    };
    const reload = () => {
      if (
        dirty &&
        !window.confirm("Reload Folio? Unsaved form changes may be lost.")
      )
        return;
      accepted = true;
      window.dispatchEvent(new Event("folio:before-reload"));
      if (activated) window.location.reload();
      else sw.messageSkipWaiting();
    };
    const show = () => {
      if (disposed) return;
      pending = true;
      if (toastId) toast.dismiss(toastId);
      toastId = `service-worker-update-${++notification}`;
      toast.message("A new version of Folio is available.", {
        id: toastId,
        duration: 10000,
        action: { label: "Reload", onClick: reload },
        cancel: { label: "Later", onClick: () => undefined },
      });
    };
    const onControlling = () => {
      activated = true;
      if (accepted) window.location.reload();
      else if (document.visibilityState === "visible") show();
      else pending = true;
    };
    const onForeground = () => {
      if (document.visibilityState !== "visible") return;
      if (pending) show();
      if (Date.now() - lastCheck < 60 * 60 * 1000) return;
      lastCheck = Date.now();
      void sw.update().catch(() => undefined);
    };
    const manualCheck = () => {
      if (pending) show();
      else
        void sw
          .update()
          .catch(() => toast.error("Could not check for updates."));
    };
    sw.addEventListener("waiting", show);
    sw.addEventListener("controlling", onControlling);
    document.addEventListener("input", markDirty);
    document.addEventListener("visibilitychange", onForeground);
    window.addEventListener("online", onForeground);
    window.addEventListener("folio:check-update", manualCheck);
    if (takeRecoveryParam()) {
      // Recovery from a stuck worker: drop its registration and don't
      // register on this load. Registering again while this page is still
      // controlled resurrects the registration being removed; the next
      // page load goes to the network instead and installs a fresh worker.
      void unregisterOurs().catch(() => undefined);
    } else {
      void sw.register().catch(() => {
        if (!disposed)
          toast.error("Offline support could not start. Reload to try again.");
      });
    }
    return () => {
      disposed = true;
      sw.removeEventListener("waiting", show);
      sw.removeEventListener("controlling", onControlling);
      document.removeEventListener("input", markDirty);
      document.removeEventListener("visibilitychange", onForeground);
      window.removeEventListener("online", onForeground);
      window.removeEventListener("folio:check-update", manualCheck);
    };
  }, []);
  return null;
}
