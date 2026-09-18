"use client";
import { useEffect, useState } from "react";
import { X } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  installInstructions,
  isAppleMobile,
  setInstallState,
  useInstall,
} from "@/lib/pwa/install";
import { toast } from "sonner";
const DISMISS_KEY = "folio:add-to-home-screen:dismissed";
/** All iOS browsers can offer installation instructions; Chromium may
 * provide a browser-owned install prompt. Settings retains the action. */
export function AddToHomeScreenBanner() {
  const { prompt, standalone } = useInstall();
  const [eligible, setEligible] = useState(false);
  const [dismissed, setDismissed] = useState(true);
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setEligible(isAppleMobile());
    try {
      setDismissed(localStorage.getItem(DISMISS_KEY) === "1");
    } catch {
      setDismissed(false);
    }
  }, []);
  if (standalone || dismissed || (!eligible && !prompt)) return null;
  const install = async () => {
    if (!prompt) return;
    try {
      await prompt.prompt();
      const choice = await prompt.userChoice;
      setInstallState({
        prompt: null,
        standalone: choice.outcome === "accepted",
      });
    } catch {
      toast.error("Use your browser’s menu to install Folio.");
    }
  };
  return (
    <div
      role="region"
      aria-label="Install Folio"
      className="bg-card border-border text-card-foreground fixed right-[max(1rem,var(--safe-right))] bottom-[var(--overlay-bottom)] left-[max(1rem,var(--safe-left))] z-40 flex items-start gap-3 rounded-lg border p-3 shadow-lg sm:mx-auto sm:max-w-md"
    >
      <div className="min-w-0 flex-1 text-sm">
        <p className="font-medium">Install Folio</p>
        {prompt ? (
          <Button className="mt-2" onClick={() => void install()}>
            Install app
          </Button>
        ) : (
          <p className="text-muted-foreground mt-1">{installInstructions()}</p>
        )}
      </div>
      <Button
        variant="ghost"
        size="icon"
        aria-label="Dismiss install hint"
        onClick={() => {
          setDismissed(true);
          try {
            localStorage.setItem(DISMISS_KEY, "1");
          } catch {
            /* Optional preference. */
          }
        }}
      >
        <X aria-hidden="true" />
      </Button>
    </div>
  );
}
