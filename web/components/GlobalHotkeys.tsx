"use client";

import { useRouter } from "next/navigation";
import { useEffect, useMemo, useState } from "react";

import { SearchModal } from "@/components/SearchModal";
import { useMe } from "@/lib/api/queries";
import {
  KEYBIND_SCOPES,
  actionForKey,
  resolveKeybinds,
  shouldSkipHotkey,
} from "@/lib/reader/keybinds";

/**
 * Mounted once at the root layout. Listens for keypresses globally and
 * fires actions whose scope is `"global"` (currently `openSettings` and
 * `openSearch`). Reader-scoped actions are dispatched by the reader's own
 * listener; this component is a no-op for them.
 *
 * Also owns the `<SearchModal />` since it controls the open-state — the
 * search hotkey toggles the same dialog wherever the user is in the app,
 * so search-from-anywhere doesn't navigate the page.
 *
 * Modifier chords (`Ctrl+K`, `Mod+,`, …) reach `actionForKey` directly:
 * the chord parser handles modifier matching. Pressing `Ctrl+K` would
 * normally focus the browser's URL bar — we `preventDefault()` on a
 * matched action to claim the keystroke first.
 */
export function GlobalHotkeys() {
  const router = useRouter();
  const me = useMe();
  const [searchOpen, setSearchOpen] = useState(false);

  const bindings = useMemo(() => {
    const stored = (me.data?.keybinds ?? null) as Record<string, string> | null;
    return resolveKeybinds(stored);
  }, [me.data?.keybinds]);

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
        setSearchOpen(true);
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
          setSearchOpen(true);
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
  }, [bindings, router]);

  return <SearchModal open={searchOpen} onOpenChange={setSearchOpen} />;
}
