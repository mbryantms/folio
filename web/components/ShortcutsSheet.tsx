"use client";

import { Fragment } from "react";

import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import {
  GLOBAL_KEYBIND_ACTIONS,
  KEYBIND_LABELS,
  READER_KEYBIND_ACTIONS,
  formatKey,
  type KeybindAction,
} from "@/lib/reader/keybinds";

/**
 * Display-only key aliases for registry actions. Values are
 * already-formatted strings (no chord parsing applied) so vim
 * conventions like `g g` render verbatim. Aliases collapse onto the
 * same row as the parent action — pressing either chord fires the
 * same action.
 */
const READER_ALIASES: Partial<Record<KeybindAction, readonly string[]>> = {
  firstPage: ["g g"],
  lastPage: ["Shift + G"],
};

const GLOBAL_ALIASES: Partial<Record<KeybindAction, readonly string[]>> = {
  openSearch: ["/"],
};

/**
 * Hard-coded entries with no registry parent. These have their own row
 * (no merging with a rebindable action above them).
 */
const ALWAYS_ON_READER: ReadonlyArray<{ keys: string[]; label: string }> = [
  { keys: ["Space"], label: "Next page (always)" },
  { keys: ["?"], label: "Show this list" },
];

const ALWAYS_ON_GLOBAL: ReadonlyArray<{ keys: string[]; label: string }> = [
  { keys: ["Alt + T"], label: "Focus latest toast (then Tab / Enter)" },
];

const MARKER_NUDGES: ReadonlyArray<{ keys: string[]; label: string }> = [
  { keys: ["Esc"], label: "Cancel the drag" },
  { keys: ["← → ↑ ↓"], label: "Nudge by 1%" },
  { keys: ["Shift + arrows"], label: "Nudge by 5%" },
];

/**
 * Keyboard shortcut reference. Bound globally to bare `?` via
 * `<GlobalShortcutsSheet>` (mounted at the root layout). Reads from the
 * resolved keymap so user overrides are reflected; Space and `?` are
 * surfaced separately because they can't be rebound. `initialSection`
 * picks the lead section so the reader gets Reader-first and everywhere
 * else gets Global-first.
 */
export function ShortcutsSheet({
  open,
  onOpenChange,
  bindings,
  initialSection = "global",
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  bindings: Record<KeybindAction, string>;
  initialSection?: "global" | "reader";
}) {
  const renderActionRow = (
    action: KeybindAction,
    aliases: Partial<Record<KeybindAction, readonly string[]>>,
  ) => {
    const primary = formatKey(bindings[action]);
    const extras = aliases[action] ?? [];
    return (
      <Row
        key={action}
        label={KEYBIND_LABELS[action]}
        keys={[primary, ...extras]}
      />
    );
  };

  const readerSection = (
    <Section title="Reader" key="reader">
      {READER_KEYBIND_ACTIONS.map((action) =>
        renderActionRow(action, READER_ALIASES),
      )}
      {ALWAYS_ON_READER.map((entry) => (
        <Row key={entry.keys[0]} label={entry.label} keys={entry.keys} />
      ))}
    </Section>
  );
  const globalSection = (
    <Section title="Global" key="global">
      {GLOBAL_KEYBIND_ACTIONS.map((action) =>
        renderActionRow(action, GLOBAL_ALIASES),
      )}
      {ALWAYS_ON_GLOBAL.map((entry) => (
        <Row key={entry.keys[0]} label={entry.label} keys={entry.keys} />
      ))}
    </Section>
  );
  const markerSection = (
    <Section
      title="While drawing a region (mouse held)"
      hint="Active only while you're mid-drag — pressing h then holding the mouse button to draw a highlight. Releases the mouse to commit; cancels the drag with Esc."
      key="markers"
    >
      {MARKER_NUDGES.map((entry) => (
        <Row key={entry.keys[0]} label={entry.label} keys={entry.keys} />
      ))}
    </Section>
  );
  const sections =
    initialSection === "reader"
      ? [readerSection, globalSection, markerSection]
      : [globalSection, readerSection, markerSection];
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="right"
        // `sm:max-w-3xl` (768px) buys room for two columns of rows
        // without crowding the chips. Overrides the Sheet primitive's
        // default `sm:max-w-sm` (384px) cap. `overflow-y-auto` stays
        // as a defensive guard for very short viewports, but at this
        // width + 2-col layout, scrolling is no longer the default.
        className="flex w-full flex-col border-neutral-800 bg-neutral-950/95 text-neutral-100 sm:max-w-3xl"
      >
        <SheetHeader>
          <SheetTitle className="text-neutral-100">
            Keyboard shortcuts
          </SheetTitle>
          <SheetDescription className="text-neutral-400">
            Customize any binding under{" "}
            <span className="text-neutral-200">Settings → Keybinds</span>.
          </SheetDescription>
        </SheetHeader>
        <div className="min-h-0 flex-1 overflow-y-auto pr-1">
          {/* Desktop (md+): two-column grid. Reader spans both rows
           *  on the left (it's the dominant section); Global + Marker
           *  stack on the right. Both columns visible at once means
           *  `initialSection` doesn't pick a "lead" here — it only
           *  steers the mobile fallback below. */}
          <div className="hidden gap-x-6 gap-y-4 md:grid md:grid-cols-2">
            <div className="md:row-span-2">{readerSection}</div>
            <div>{globalSection}</div>
            <div>{markerSection}</div>
          </div>
          {/* Mobile (single column): respect `initialSection` so the
           *  reader's most-used shortcuts land first in-context. */}
          <div className="space-y-4 md:hidden">{sections}</div>
        </div>
      </SheetContent>
    </Sheet>
  );
}

function Section({
  title,
  hint,
  children,
}: {
  title: string;
  hint?: string;
  children: React.ReactNode;
}) {
  // No `mt-6` — vertical spacing is owned by the parent (grid
  // `gap-y-4` on desktop, `space-y-4` on mobile). Per-section margin
  // here would double up against the parent gap.
  return (
    <div>
      <h3 className="mb-2 text-xs font-semibold tracking-wide text-neutral-500 uppercase">
        {title}
      </h3>
      {hint ? (
        <p className="mb-2 text-xs leading-relaxed text-neutral-400">{hint}</p>
      ) : null}
      <ul className="space-y-2">{children}</ul>
    </div>
  );
}

function Row({ label, keys }: { label: string; keys: readonly string[] }) {
  return (
    <li className="flex items-center justify-between gap-3 rounded border border-neutral-800/60 bg-neutral-900/50 px-3 py-2 text-sm">
      <span className="text-neutral-200">{label}</span>
      <span className="ml-auto inline-flex shrink-0 items-center gap-1.5">
        {keys.map((k, i) => (
          <Fragment key={i}>
            {i > 0 ? (
              <span className="text-[10px] text-neutral-500">or</span>
            ) : null}
            <kbd className="inline-flex min-w-8 items-center justify-center rounded border border-neutral-700 bg-neutral-950 px-2 py-0.5 font-mono text-xs text-neutral-300">
              {k}
            </kbd>
          </Fragment>
        ))}
      </span>
    </li>
  );
}
