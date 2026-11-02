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

import dynamic from "next/dynamic";
import { useMe } from "@/lib/api/queries";
import {
  readMeKeybinds,
  resolveKeybinds,
  shouldSkipHotkey,
} from "@/lib/reader/keybinds";

/** The sheet (Radix Dialog + scroll lock + focus trap + the keybind
 *  tables) is only needed once someone presses `?` or picks "Keyboard
 *  shortcuts". This provider wraps the root layout, so a static import
 *  would put all of it in every route's first-load JS — including the
 *  reader's budgeted bundle (WP-4.4). Lazy-load it and mount on first
 *  open; the open state is already set, so the sheet appears as soon as
 *  the chunk resolves. */
const ShortcutsSheet = dynamic(
  () => import("./ShortcutsSheet").then((m) => m.ShortcutsSheet),
  { ssr: false },
);

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
  // Mount the lazy sheet on first open and keep it mounted afterwards so
  // the close animation plays and re-opens are instant.
  const [hasOpened, setHasOpened] = useState(false);
  if (isOpen && !hasOpened) setHasOpened(true);
  const me = useMe();
  const pathname = usePathname() ?? "";

  const meKeybinds = readMeKeybinds(me);
  const bindings = useMemo(() => resolveKeybinds(meKeybinds), [meKeybinds]);

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
      {hasOpened ? (
        <ShortcutsSheet
          open={isOpen}
          onOpenChange={setOpen}
          bindings={bindings}
          initialSection={inReader ? "reader" : "global"}
        />
      ) : null}
    </Ctx.Provider>
  );
}
