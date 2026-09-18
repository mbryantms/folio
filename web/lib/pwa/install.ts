"use client";
import { useSyncExternalStore } from "react";
export interface InstallPrompt extends Event {
  prompt(): Promise<void>;
  userChoice: Promise<{ outcome: "accepted" | "dismissed" }>;
}
const initial = { prompt: null as InstallPrompt | null, standalone: false };
let state = initial;
const listeners = new Set<() => void>();
export function setInstallState(next: typeof state) {
  state = next;
  listeners.forEach((listener) => listener());
}
export function useInstall() {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    () => state,
    () => initial,
  );
}
export function isAppleMobile() {
  return (
    /iP(hone|ad|od)/.test(navigator.userAgent) ||
    (/Macintosh/.test(navigator.userAgent) && navigator.maxTouchPoints > 1)
  );
}
export function installInstructions() {
  if (isAppleMobile())
    return "Open your browser’s Share menu, then choose Add to Home Screen. Folio opens in its own app window.";
  if (
    /Macintosh/.test(navigator.userAgent) &&
    /Safari/.test(navigator.userAgent) &&
    !/Chrome/.test(navigator.userAgent)
  )
    return "In Safari, choose File → Add to Dock.";
  return "Open your browser’s menu and look for Install app or Add to Home Screen. Availability depends on your browser.";
}
