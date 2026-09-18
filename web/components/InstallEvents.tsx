"use client";
import { useEffect } from "react";
import { setInstallState, type InstallPrompt } from "@/lib/pwa/install";
import { isStandaloneDisplay } from "@/lib/use-pull-to-refresh";
export function InstallEvents() {
  useEffect(() => {
    const media = window.matchMedia("(display-mode: standalone)");
    let prompt: InstallPrompt | null = null;
    const update = () =>
      setInstallState({ prompt, standalone: isStandaloneDisplay() });
    const available = (event: Event) => {
      event.preventDefault();
      prompt = event as InstallPrompt;
      update();
    };
    const installed = () => {
      prompt = null;
      setInstallState({ prompt, standalone: true });
    };
    update();
    window.addEventListener("beforeinstallprompt", available);
    window.addEventListener("appinstalled", installed);
    media.addEventListener("change", update);
    return () => {
      window.removeEventListener("beforeinstallprompt", available);
      window.removeEventListener("appinstalled", installed);
      media.removeEventListener("change", update);
    };
  }, []);
  return null;
}
