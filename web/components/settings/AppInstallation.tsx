"use client";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { SettingsSection } from "./SettingsSection";
import {
  installInstructions,
  setInstallState,
  useInstall,
} from "@/lib/pwa/install";
export function AppInstallation() {
  const { prompt, standalone } = useInstall();
  const [instructions, setInstructions] = useState("");
  const install = async () => {
    if (!prompt) {
      setInstructions(installInstructions());
      return;
    }
    try {
      await prompt.prompt();
      const choice = await prompt.userChoice;
      setInstallState({
        prompt: null,
        standalone: choice.outcome === "accepted",
      });
    } catch {
      toast.error("Could not open installation. Try your browser’s menu.");
    }
  };
  return (
    <SettingsSection
      title="Folio app"
      description="Install Folio for easy access from your home screen or dock."
    >
      <div className="flex flex-wrap gap-3">
        <Button onClick={() => void install()} disabled={standalone}>
          {standalone ? "Running as an app" : "Install Folio"}
        </Button>
        <Button
          variant="outline"
          onClick={() => window.dispatchEvent(new Event("folio:check-update"))}
        >
          Check for updates
        </Button>
      </div>
      {instructions && (
        <p role="status" className="mt-3 text-sm">
          {instructions}
        </p>
      )}
    </SettingsSection>
  );
}
