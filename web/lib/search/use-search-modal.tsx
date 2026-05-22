"use client";

import { createContext, useContext, useMemo, useState } from "react";

import { SearchModal } from "@/components/SearchModal";

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
  const value = useMemo(() => ({ open, setOpen }), [open]);
  return (
    <Ctx.Provider value={value}>
      {children}
      <SearchModal open={open} onOpenChange={setOpen} />
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
