"use client";

import { usePathname } from "next/navigation";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";

import { ShortcutsSheet } from "./ShortcutsSheet";
import { useMe } from "@/lib/api/queries";
import { resolveKeybinds, shouldSkipHotkey } from "@/lib/reader/keybinds";

interface ShortcutsSheetContextValue {
  open: () => void;
  close: () => void;
  toggle: () => void;
  isOpen: boolean;
}

const Ctx = createContext<ShortcutsSheetContextValue | null>(null);

/**
 * Read the global shortcuts-sheet open/close affordances. Safe to call
 * from anywhere under `<GlobalShortcutsSheet>` (which wraps the root
 * layout). Returns no-op handlers when no provider is mounted (e.g.
 * sign-in / register routes that bypass the signed-in shell).
 */
export function useShortcutsSheet(): ShortcutsSheetContextValue {
  const v = useContext(Ctx);
  return (
    v ?? {
      open: () => undefined,
      close: () => undefined,
      toggle: () => undefined,
      isOpen: false,
    }
  );
}

/**
 * Mounts the global keyboard-shortcuts sheet and listens for bare `?`
 * to toggle it. Provides a context so the user-menu entry and any
 * sidebar help button can open the same sheet without duplicating the
 * state. Section ordering picks Reader-first inside `/read/...`, else
 * Global-first — so the relevant block is what the user sees first.
 */
export function GlobalShortcutsSheet({
  children,
}: {
  children: React.ReactNode;
}) {
  const [isOpen, setOpen] = useState(false);
  const me = useMe();
  const pathname = usePathname() ?? "";

  const bindings = useMemo(() => {
    const stored = (me.data?.keybinds ?? null) as Record<string, string> | null;
    return resolveKeybinds(stored);
  }, [me.data?.keybinds]);

  const open = useCallback(() => setOpen(true), []);
  const close = useCallback(() => setOpen(false), []);
  const toggle = useCallback(() => setOpen((v) => !v), []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (shouldSkipHotkey(e)) return;
      // Bare `?` — typically `Shift+/`. Don't also fire on Mod+? so a
      // future hotkey collision doesn't surprise. Hard-coded (not in
      // the keybind registry) so the help surface that lists bindings
      // doesn't itself have a customizable binding.
      if (e.key === "?" && !e.metaKey && !e.ctrlKey && !e.altKey) {
        e.preventDefault();
        toggle();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggle]);

  const inReader = pathname.includes("/read/");

  const value = useMemo<ShortcutsSheetContextValue>(
    () => ({ open, close, toggle, isOpen }),
    [open, close, toggle, isOpen],
  );

  return (
    <Ctx.Provider value={value}>
      {children}
      <ShortcutsSheet
        open={isOpen}
        onOpenChange={setOpen}
        bindings={bindings}
        initialSection={inReader ? "reader" : "global"}
      />
    </Ctx.Provider>
  );
}
