"use client";

/* Folio wrapper over the vendored shadcn Toaster (components/ui is
 * pristine): top-centre on phones (bottom toasts covered bottom-sheet
 * dialog buttons and the tab bar on iOS), bottom-right otherwise; offsets
 * clear the safe areas and the bottom tab bar; the amber theme maps to
 * Sonner's light palette (its theme union has no "amber"). The vendored
 * file resolves the next-themes theme first and then spreads our props, so
 * the mapping here wins. The action / cancel buttons inside a toast are
 * styled from `globals.css` (Sonner's injected stylesheet is unlayered, so
 * `toastOptions.classNames` utilities could not beat it). App code imports
 * from here, never from components/ui (ESLint enforces it). */
import { useTheme } from "next-themes";
import * as React from "react";
import type { ToasterProps } from "sonner";

import { Toaster as UiToaster } from "@/components/ui/sonner";
import { useIsPhone } from "@/lib/ui/use-is-phone";

export function Toaster(props: ToasterProps) {
  const { theme = "dark" } = useTheme();
  const phone = useIsPhone();
  const sonnerTheme: ToasterProps["theme"] =
    theme === "light" || theme === "dark" || theme === "system"
      ? theme
      : "light";
  return (
    <UiToaster
      theme={sonnerTheme}
      hotkey={["altKey", "KeyT"]}
      position={phone ? "top-center" : "bottom-right"}
      style={
        {
          // Restated from the vendored file: it spreads our props after its
          // own `style`, so this object replaces upstream's token mapping
          // instead of adding to it. Without these Sonner falls back to its
          // own palette (black / white toasts, 8px corners).
          "--normal-bg": "var(--popover)",
          "--normal-text": "var(--popover-foreground)",
          "--normal-border": "var(--border)",
          "--border-radius": "var(--radius)",
          // The close button's dark-theme hover pair (Sonner hard-codes
          // near-black otherwise).
          "--normal-bg-hover": "var(--muted)",
          "--normal-border-hover": "var(--border)",
          "--offset-bottom": "var(--overlay-bottom)",
          "--offset-top": "max(16px, var(--safe-top, 0px))",
          "--offset-right": "max(16px, var(--safe-right))",
          "--offset-left": "max(16px, var(--safe-left))",
          "--mobile-offset-bottom": "var(--overlay-bottom)",
          "--mobile-offset-top": "max(16px, var(--safe-top, 0px))",
          "--mobile-offset-right": "max(16px, var(--safe-right))",
          "--mobile-offset-left": "max(16px, var(--safe-left))",
        } as React.CSSProperties
      }
      duration={4000}
      expand={false}
      visibleToasts={3}
      closeButton
      {...props}
    />
  );
}
