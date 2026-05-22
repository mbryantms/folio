"use client";

import { Share, X } from "lucide-react";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { isStandaloneDisplay } from "@/lib/use-pull-to-refresh";

/**
 * Dismissible "Add to Home Screen" helper for iOS Safari.
 *
 * iOS does not fire `beforeinstallprompt`. There is no Apple-side
 * UI affordance that tells the user the page can be installed —
 * the Share-sheet "Add to Home Screen" action is the only path,
 * and most users never discover it. This banner is the in-app
 * pointer to that action.
 *
 * Show conditions (all must be true):
 *
 * - Running on iOS Safari (other browsers either auto-prompt or
 *   don't honour the share-sheet path at all).
 * - Not already in standalone display mode (i.e. the app is not
 *   already installed).
 * - The user has not previously dismissed the banner. The dismiss
 *   choice is stored in `localStorage` under a single key, so a
 *   browser data clear or a new device re-shows it.
 *
 * Mounted from `MainShell` so it appears on the post-auth library
 * surface. Intentionally not mounted on the sign-in or admin
 * shells: installing before signing in would land an
 * unauthenticated install on the Home Screen, and admins reaching
 * the admin shell are by definition already-authenticated power
 * users who do not need the nudge.
 */

const DISMISS_KEY = "folio:add-to-home-screen:dismissed";

function isIosSafari(): boolean {
  if (typeof navigator === "undefined") return false;
  const ua = navigator.userAgent;
  // iPadOS 13+ reports itself as Mac in `userAgent` — fall back
  // to the touch + Safari heuristic.
  const isIosUa = /iP(hone|od|ad)/.test(ua);
  const isMacWithTouch =
    /Macintosh/.test(ua) && navigator.maxTouchPoints > 1;
  if (!isIosUa && !isMacWithTouch) return false;
  // Chrome / Firefox / Edge on iOS all use the WebKit engine but
  // identify in the UA. Add-to-Home-Screen via share sheet is
  // available only in Safari; the third-party browsers either
  // ship their own (Chrome) or do not support it at all.
  const isInAppBrowser = /CriOS|FxiOS|EdgiOS|OPiOS|YaBrowser/.test(ua);
  return !isInAppBrowser;
}

export function AddToHomeScreenBanner() {
  // Server-render nothing. The decision needs `navigator`,
  // `localStorage`, and `matchMedia`, which only exist in the
  // browser. Returning `null` on the first client render and then
  // flipping after the effect avoids a hydration mismatch.
  const [show, setShow] = useState(false);

  useEffect(() => {
    if (!isIosSafari()) return;
    if (isStandaloneDisplay()) return;
    try {
      if (window.localStorage.getItem(DISMISS_KEY) === "1") return;
    } catch {
      // Storage access can throw under Lockdown Mode or in
      // Safari Private Browsing in some configurations. Treat
      // a throw as "not dismissed" so the banner still shows;
      // the dismiss button below will simply no-op on click.
    }
    // One-shot client-only feature detection. The SSR render is
    // intentionally hidden (so no hydration mismatch); the effect
    // flips the banner in once we can actually inspect navigator
    // / localStorage / matchMedia. Same pattern as the existing
    // mount-flag callers (see Chrome.tsx, MarkerEditor.tsx,
    // ReaderSettings.tsx).
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setShow(true);
  }, []);

  const dismiss = () => {
    setShow(false);
    try {
      window.localStorage.setItem(DISMISS_KEY, "1");
    } catch {
      // See note in the effect — losing the dismissal across
      // navigation is annoying but not broken.
    }
  };

  if (!show) return null;

  return (
    <div
      role="region"
      aria-label="Install Folio"
      className="bg-card border-border text-card-foreground fixed bottom-[max(1rem,var(--safe-bottom))] left-[max(1rem,var(--safe-left))] right-[max(1rem,var(--safe-right))] z-40 flex items-start gap-3 rounded-lg border p-3 shadow-lg sm:mx-auto sm:max-w-md"
    >
      <Share className="text-muted-foreground mt-0.5 h-5 w-5 shrink-0" aria-hidden="true" />
      <div className="min-w-0 flex-1 text-sm">
        <p className="font-medium">Install Folio on your Home Screen.</p>
        <p className="text-muted-foreground mt-1">
          Tap the Share icon in Safari&apos;s toolbar, then choose{" "}
          <span className="font-medium">Add to Home Screen</span>. The app
          will launch full-screen the next time you open it.
        </p>
      </div>
      <Button
        variant="ghost"
        size="icon"
        className="shrink-0"
        aria-label="Dismiss install hint"
        onClick={dismiss}
      >
        <X className="h-4 w-4" />
      </Button>
    </div>
  );
}
