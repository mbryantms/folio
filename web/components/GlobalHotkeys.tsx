"use client";

import { useRouter } from "next/navigation";
import { useEffect, useMemo } from "react";

import { useMe } from "@/lib/api/queries";
import {
  KEYBIND_SCOPES,
  actionForKey,
  readMeKeybinds,
  resolveKeybinds,
  shouldSkipHotkey,
} from "@/lib/reader/keybinds";
import { useSearchModal } from "@/lib/search/use-search-modal";

/**
 * Mounted once at the root layout. Listens for keypresses globally and
 * fires actions whose scope is `"global"` (currently `openSettings` and
 * `openSearch`). Reader-scoped actions are dispatched by the reader's own
 * listener; this component is a no-op for them.
 *
 * The `<SearchModal />` itself is owned by `<SearchModalProvider>` so a
 * topbar trigger can open it without prop-drilling; this hook just flips
 * the shared open-state.
 *
 * Modifier chords (`Ctrl+K`, `Mod+,`, …) reach `actionForKey` directly:
 * the chord parser handles modifier matching. Pressing `Ctrl+K` would
 * normally focus the browser's URL bar — we `preventDefault()` on a
 * matched action to claim the keystroke first.
 */
export function GlobalHotkeys() {
  const router = useRouter();
  const me = useMe();
  const { setOpen } = useSearchModal();

  const meKeybinds = readMeKeybinds(me);
  const bindings = useMemo(() => resolveKeybinds(meKeybinds), [meKeybinds]);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      // Don't hijack typing in form fields or rich-text surfaces. Reader
      // and the search input apply the same gate via their own listeners.
      if (shouldSkipHotkey(e)) return;

      // Bare `/` opens search — common web convention (GitHub / YouTube /
      // Discord). Not in the keybind registry: it's an alias for
      // `openSearch`, not a rebindable action. Users who rebind
      // `openSearch` still keep `/` as a fixed alias.
      if (
        e.key === "/" &&
        !e.metaKey &&
        !e.ctrlKey &&
        !e.altKey &&
        !e.shiftKey
      ) {
        e.preventDefault();
        setOpen(true);
        return;
      }

      const action = actionForKey(e, bindings);
      if (!action || KEYBIND_SCOPES[action] !== "global") return;
      switch (action) {
        case "openSettings":
          e.preventDefault();
          router.push("/settings");
          break;
        case "openSearch":
          e.preventDefault();
          setOpen(true);
          break;
        // `toggleSidebar` is global-scoped for discoverability (so it
        // appears in Settings → Keybinds + the shortcuts sheet) but the
        // sidebar hook owns its dispatch. Fall through here so we don't
        // claim the keystroke before the sidebar listener sees it.
        default:
          break;
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [bindings, router, setOpen]);

  return null;
}
