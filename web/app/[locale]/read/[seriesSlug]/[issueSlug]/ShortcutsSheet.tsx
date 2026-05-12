"use client";

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

const ALWAYS_ON: ReadonlyArray<{ key: string; label: string }> = [
  { key: "Space", label: "Next page (always)" },
  { key: "?", label: "Show this list" },
];

/**
 * Keyboard shortcut reference (M6). Bound globally to `?` in the reader.
 * Reads from the resolved keymap so user overrides are reflected; Space
 * and `?` themselves are surfaced separately because they can't be
 * rebound. Sectioned into Reader (the relevant set when the sheet is
 * open) and Global (works anywhere) so users see what applies in context.
 */
export function ShortcutsSheet({
  open,
  onOpenChange,
  bindings,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  bindings: Record<KeybindAction, string>;
}) {
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="right"
        className="w-80 border-neutral-800 bg-neutral-950/95 text-neutral-100"
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
        <Section title="Reader">
          {READER_KEYBIND_ACTIONS.map((action) => (
            <Row
              key={action}
              label={KEYBIND_LABELS[action]}
              keyName={bindings[action]}
            />
          ))}
          {ALWAYS_ON.map((entry) => (
            <Row key={entry.key} label={entry.label} keyName={entry.key} />
          ))}
        </Section>
        <Section title="Global">
          {GLOBAL_KEYBIND_ACTIONS.map((action) => (
            <Row
              key={action}
              label={KEYBIND_LABELS[action]}
              keyName={bindings[action]}
            />
          ))}
        </Section>
      </SheetContent>
    </Sheet>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="mt-6">
      <h3 className="mb-2 text-xs font-semibold tracking-wide text-neutral-500 uppercase">
        {title}
      </h3>
      <ul className="space-y-2">{children}</ul>
    </div>
  );
}

function Row({ label, keyName }: { label: string; keyName: string }) {
  return (
    <li className="flex items-center justify-between gap-3 rounded border border-neutral-800/60 bg-neutral-900/50 px-3 py-2 text-sm">
      <span className="text-neutral-200">{label}</span>
      <kbd className="inline-flex min-w-8 items-center justify-center rounded border border-neutral-700 bg-neutral-950 px-2 py-0.5 font-mono text-xs text-neutral-300">
        {formatKey(keyName)}
      </kbd>
    </li>
  );
}
