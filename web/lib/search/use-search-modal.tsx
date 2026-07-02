"use client";

import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
} from "react";
import dynamic from "next/dynamic";

/** `SearchModal` pulls in cmdk (~several KB). The provider mounts at the
 *  root layout, so a static import would fold cmdk into the app-wide shared
 *  first-load bundle — including the reader route, which has a tight budget
 *  (§18.1) and never surfaces the modal until the user hits ⌘K. Lazy-load it
 *  and gate the mount on first-open so the chunk is fetched only when the
 *  dialog is actually summoned. `ssr: false` because it's purely interactive
 *  client UI with nothing to render on the server. */
const SearchModal = dynamic(
  () => import("@/components/SearchModal").then((m) => m.SearchModal),
  { ssr: false },
);

/** Owns the global search dialog's open-state so any topbar trigger, hotkey
 *  handler, or programmatic caller can open it without prop-drilling. The
 *  modal itself is rendered once by the provider; consumers only flip the
 *  open boolean.
 *
 *  Mounted at the root layout, so `useSearchModal()` is available anywhere
 *  in the client tree. */
type SearchModalCtx = {
  open: boolean;
  setOpen: (next: boolean) => void;
};

const Ctx = createContext<SearchModalCtx | null>(null);

export function SearchModalProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  // Don't render (and thus don't fetch) the lazy chunk until the dialog has
  // been opened at least once. After that we keep it mounted so the
  // close/exit animation plays and re-opens are instant.
  const [hasOpened, setHasOpened] = useState(false);
  const setOpenTracked = useCallback((next: boolean) => {
    if (next) setHasOpened(true);
    setOpen(next);
  }, []);
  const value = useMemo(
    () => ({ open, setOpen: setOpenTracked }),
    [open, setOpenTracked],
  );
  return (
    <Ctx.Provider value={value}>
      {children}
      {hasOpened && <SearchModal open={open} onOpenChange={setOpenTracked} />}
    </Ctx.Provider>
  );
}

/** Consumers should call `setOpen(true)` to surface the global search
 *  dialog. Throws when used outside the provider — that signals a
 *  missing layout-level mount, not a runtime fallback worth handling. */
export function useSearchModal(): SearchModalCtx {
  const ctx = useContext(Ctx);
  if (!ctx) {
    throw new Error("useSearchModal must be used inside <SearchModalProvider>");
  }
  return ctx;
}
