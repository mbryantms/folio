"use client";

import { useTheme } from "next-themes";
import { usePathname, useRouter } from "next/navigation";
import { useCallback, useMemo } from "react";

import { useUpdatePreferences } from "@/lib/api/mutations";
import { useContinueReading, useMe } from "@/lib/api/queries";
import type { ContinueReadingView } from "@/lib/api/types";
import {
  formatKey,
  readMeKeybinds,
  resolveKeybinds,
} from "@/lib/reader/keybinds";
import {
  SEARCH_ACTIONS_CAP,
  rankSearchActions,
  suggestedSearchActions,
  type SearchAction,
} from "@/lib/search/actions-registry";
import { writeThemeCookie } from "@/lib/theme";
import {
  requestShortcutsSheet,
  requestSidebarToggle,
} from "@/lib/ui/shell-events";
import { readerUrl } from "@/lib/urls";

/** The theme the toggle switches to. Dark-family themes (dark, amber)
 *  go light; light goes dark. An unresolved theme (pre-hydration) goes
 *  light, matching the dark server fallback. */
export function nextToggleTheme(
  resolved: string | undefined,
): "dark" | "light" {
  return resolved === "light" ? "dark" : "light";
}

/** Ids the current context can't run: no sidebar outside the shells (the
 *  reader), nothing to continue without an in-progress issue. */
export function unavailableActionIds({
  pathname,
  hasContinue,
}: {
  pathname: string;
  hasContinue: boolean;
}): Set<string> {
  const out = new Set<string>();
  if (pathname === "/read" || pathname.startsWith("/read/")) {
    out.add("cmd-toggle-sidebar");
  }
  if (!hasContinue) out.add("cmd-continue-reading");
  return out;
}

type ContinueCard = ContinueReadingView["items"][number];

function continueDetail(card: ContinueCard): string {
  const num = card.issue.number ? ` #${card.issue.number}` : "";
  const pct = Math.round((card.progress.percent ?? 0) * 100);
  return `${card.series_name}${num} · ${pct}%`;
}

/**
 * Shared palette behaviour for both search surfaces (the ⌘K dialog and
 * the desktop topbar dropdown): which actions to list for a needle, the
 * subtitle and key hint per row, and how to run one.
 *
 * `active` gates the continue-reading fetch to while the surface is open
 * (the query is shared with the Home rail, so it is usually cached).
 */
export function useSearchActions({
  needle,
  commandMode,
  active,
}: {
  needle: string;
  commandMode: boolean;
  active: boolean;
}) {
  const router = useRouter();
  const pathname = usePathname() ?? "";
  const me = useMe();
  const role = me.data?.role;
  const continueQ = useContinueReading({ enabled: active && !!me.data });
  const continueCard = continueQ.data?.items[0] ?? null;
  const { resolvedTheme, setTheme } = useTheme();
  const updatePrefs = useUpdatePreferences({ silent: true });

  const meKeybinds = readMeKeybinds(me);
  const bindings = useMemo(() => resolveKeybinds(meKeybinds), [meKeybinds]);

  const exclude = useMemo(
    () => unavailableActionIds({ pathname, hasContinue: !!continueCard }),
    [pathname, continueCard],
  );

  const trimmed = needle.trim();
  const actions = useMemo<readonly SearchAction[]>(() => {
    if (!commandMode && trimmed.length === 0) {
      return suggestedSearchActions(role, { exclude });
    }
    const ranked = rankSearchActions(trimmed, role, { exclude });
    // Command mode is the full command surface; mixed with content hits
    // the actions stay a short lead-in.
    return commandMode ? ranked : ranked.slice(0, SEARCH_ACTIONS_CAP);
  }, [commandMode, trimmed, role, exclude]);

  /** Row subtitle: the resumed issue for continue-reading, else the group. */
  const detailFor = useCallback(
    (a: SearchAction): string =>
      a.command === "continue-reading" && continueCard
        ? continueDetail(continueCard)
        : a.group,
    [continueCard],
  );

  /** Key hint text, or null when the action has no shortcut. */
  const keyHintFor = useCallback(
    (a: SearchAction): string | null => {
      if (a.keybind) return formatKey(bindings[a.keybind]);
      return a.keyHint ?? null;
    },
    [bindings],
  );

  /** Carry out an action. Callers close their surface first. */
  const run = useCallback(
    (a: SearchAction) => {
      if (a.href) {
        router.push(a.href);
        return;
      }
      switch (a.command) {
        case "continue-reading":
          if (continueCard) router.push(readerUrl(continueCard.issue));
          break;
        case "toggle-theme": {
          const next = nextToggleTheme(resolvedTheme);
          writeThemeCookie(next);
          setTheme(next);
          updatePrefs.mutate({ theme: next });
          break;
        }
        case "toggle-sidebar":
          requestSidebarToggle();
          break;
        case "show-shortcuts":
          requestShortcutsSheet();
          break;
        default:
          break;
      }
    },
    [continueCard, resolvedTheme, router, setTheme, updatePrefs],
  );

  return { actions, run, detailFor, keyHintFor };
}
