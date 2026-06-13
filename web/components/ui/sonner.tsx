"use client";

import { useTheme } from "next-themes";
import { Toaster as Sonner } from "sonner";

type ToasterProps = React.ComponentProps<typeof Sonner>;

const Toaster = ({ ...props }: ToasterProps) => {
  const { theme = "dark" } = useTheme();
  // Sonner's union is light|dark|system; the app also ships an
  // "amber" theme (light-surfaced). Casting it through made Sonner's
  // internal defaults (icons, close button) fall back unpredictably —
  // map it to its light base instead.
  const sonnerTheme: ToasterProps["theme"] =
    theme === "light" || theme === "dark" || theme === "system"
      ? theme
      : "light";

  return (
    <Sonner
      theme={sonnerTheme}
      // All Sonner props below are pinned to current defaults rather
      // than left implicit. The point isn't to change behavior — it's
      // to make a sonner upgrade safe (no silent default-shift) and to
      // give product a single one-line edit when they want to retune.
      // Notifications cleanup M0 finalization.
      hotkey={["altKey", "KeyT"]}
      position="bottom-right"
      // Push the toast stack inside the iOS home-indicator + landscape
      // notch in PWA standalone mode. Sonner positions its container
      // via these CSS custom properties — overriding here keeps the
      // 16px floor while expanding as the safe-area inset grows.
      style={
        {
          "--offset-bottom": "max(16px, var(--safe-bottom))",
          "--offset-right": "max(16px, var(--safe-right))",
          "--offset-left": "max(16px, var(--safe-left))",
          "--mobile-offset-bottom": "max(16px, var(--safe-bottom))",
          "--mobile-offset-right": "max(16px, var(--safe-right))",
          "--mobile-offset-left": "max(16px, var(--safe-left))",
        } as React.CSSProperties
      }
      duration={4000}
      expand={false}
      // Cap queue depth so a scan-completion burst (10+ thumbnail
      // updates) can't stack toasts past the visible viewport. Older
      // toasts fall off the bottom; the most recent 3 stay visible.
      visibleToasts={3}
      // Adds the X icon for manual dismiss. Important for the longer
      // error / Undo toasts (8 s) where the user may want to dismiss
      // before the timeout.
      closeButton
      className="toaster group"
      toastOptions={{
        classNames: {
          toast:
            "group toast group-[.toaster]:bg-background group-[.toaster]:text-foreground group-[.toaster]:border-border group-[.toaster]:shadow-lg",
          description: "group-[.toast]:text-muted-foreground",
          actionButton:
            "group-[.toast]:bg-primary group-[.toast]:text-primary-foreground",
          cancelButton:
            "group-[.toast]:bg-muted group-[.toast]:text-muted-foreground",
        },
      }}
      {...props}
    />
  );
};

export { Toaster };
