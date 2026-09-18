"use client";
import { useEffect } from "react";
import { create } from "zustand";

export const useWakePreference = create<{
  enabled: boolean;
  setEnabled: (enabled: boolean) => void;
}>((set) => ({
  enabled: false,
  setEnabled: (enabled) => {
    set({ enabled });
    try {
      localStorage.setItem("folio:keep-awake", String(enabled));
    } catch {
      /* Optional preference. */
    }
  },
}));

/** Only mounted by the reader. The OS may release or deny a lock at any time. */
export function useReaderWakeLock() {
  const enabled = useWakePreference((state) => state.enabled);
  useEffect(() => {
    try {
      useWakePreference.setState({
        enabled: localStorage.getItem("folio:keep-awake") === "true",
      });
    } catch {
      /* Optional preference. */
    }
  }, []);
  useEffect(() => {
    if (!enabled || !("wakeLock" in navigator)) return;
    let disposed = false;
    let acquiring = false;
    let lock: WakeLockSentinel | undefined;
    const acquire = async () => {
      if (
        disposed ||
        document.visibilityState !== "visible" ||
        lock ||
        acquiring
      )
        return;
      acquiring = true;
      try {
        const next = await navigator.wakeLock.request("screen");
        if (disposed || document.visibilityState !== "visible") {
          await next.release();
          return;
        }
        lock = next;
        next.addEventListener("release", () => {
          if (lock === next) lock = undefined;
        });
      } catch {
        /* Battery policy / permission denied: normal screen behavior. */
      } finally {
        acquiring = false;
      }
    };
    const visibility = () => {
      if (document.visibilityState === "visible") void acquire();
      else {
        void lock?.release();
        lock = undefined;
      }
    };
    void acquire();
    document.addEventListener("visibilitychange", visibility);
    return () => {
      disposed = true;
      document.removeEventListener("visibilitychange", visibility);
      void lock?.release();
    };
  }, [enabled]);
}
