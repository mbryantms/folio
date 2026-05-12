"use client";

import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Skeleton } from "@/components/ui/skeleton";
import { useMe } from "@/lib/api/queries";
import { useUpdatePreferences } from "@/lib/api/mutations";
import {
  GLOBAL_KEYBIND_ACTIONS,
  KEYBIND_DEFAULTS,
  KEYBIND_LABELS,
  READER_KEYBIND_ACTIONS,
  comboFromEvent,
  formatKey,
  resolveKeybinds,
  type KeybindAction,
} from "@/lib/reader/keybinds";

import { SettingsSection } from "./SettingsSection";

export function KeybindEditor() {
  const me = useMe();
  const update = useUpdatePreferences();
  const [editing, setEditing] = useState<KeybindAction | null>(null);

  if (me.isLoading) return <Skeleton className="h-72 w-full" />;
  if (me.error || !me.data) {
    return (
      <p className="text-destructive text-sm">Failed to load preferences.</p>
    );
  }

  const stored = (me.data.keybinds ?? {}) as Record<string, string>;
  const resolved = resolveKeybinds(stored);

  function setBinding(action: KeybindAction, key: string) {
    const next = { ...stored, [action]: key };
    update.mutate({ keybinds: next });
    setEditing(null);
  }
  function clearBinding(action: KeybindAction) {
    const next = { ...stored };
    delete next[action];
    update.mutate({ keybinds: next });
  }
  function resetAll() {
    update.mutate({ keybinds: {} });
  }

  return (
    <>
      <SettingsSection
        title="Global hotkeys"
        description="Active anywhere in the app. Skipped while typing in form fields."
      >
        <BindingList
          actions={GLOBAL_KEYBIND_ACTIONS}
          stored={stored}
          resolved={resolved}
          onEdit={setEditing}
          onClear={clearBinding}
        />
      </SettingsSection>
      <SettingsSection
        title="Reader hotkeys"
        description="Active inside the reader. Spacebar always advances and is not user-rebindable."
      >
        <BindingList
          actions={READER_KEYBIND_ACTIONS}
          stored={stored}
          resolved={resolved}
          onEdit={setEditing}
          onClear={clearBinding}
        />
        <div className="mt-4 flex justify-end">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={Object.keys(stored).length === 0 || update.isPending}
            onClick={resetAll}
          >
            Reset to defaults
          </Button>
        </div>
      </SettingsSection>
      <CaptureDialog
        action={editing}
        onCancel={() => setEditing(null)}
        onCapture={(key) => editing && setBinding(editing, key)}
      />
    </>
  );
}

/** Renders one set of action rows. Stateless — used for both scopes. */
function BindingList({
  actions,
  stored,
  resolved,
  onEdit,
  onClear,
}: {
  actions: readonly KeybindAction[];
  stored: Record<string, string>;
  resolved: Record<KeybindAction, string>;
  onEdit: (action: KeybindAction) => void;
  onClear: (action: KeybindAction) => void;
}) {
  return (
    <ul className="divide-border divide-y">
      {actions.map((action) => {
        const isOverridden = typeof stored[action] === "string";
        const current = resolved[action];
        return (
          <li
            key={action}
            className="flex items-center justify-between gap-4 py-3"
          >
            <div className="min-w-0">
              <p className="text-foreground text-sm font-medium">
                {KEYBIND_LABELS[action]}
              </p>
              <p className="text-muted-foreground text-xs">
                Default: {formatKey(KEYBIND_DEFAULTS[action])}
              </p>
            </div>
            <div className="flex items-center gap-2">
              <button
                type="button"
                aria-label={`Change binding for ${KEYBIND_LABELS[action]}`}
                onClick={() => onEdit(action)}
                className="border-input bg-background hover:bg-secondary rounded border px-3 py-1.5 text-sm font-medium"
              >
                {formatKey(current)}
              </button>
              {isOverridden ? (
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  onClick={() => onClear(action)}
                >
                  Clear
                </Button>
              ) : null}
            </div>
          </li>
        );
      })}
    </ul>
  );
}

/**
 * Modal that listens for the next keyboard event and reports the captured
 * `KeyboardEvent.key`. Escape always cancels (so users can back out without
 * being trapped). Modifier-only presses are ignored.
 */
function CaptureDialog({
  action,
  onCancel,
  onCapture,
}: {
  action: KeybindAction | null;
  onCancel: () => void;
  onCapture: (key: string) => void;
}) {
  return (
    <Dialog open={action !== null} onOpenChange={(open) => !open && onCancel()}>
      <DialogContent className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>Press a key</DialogTitle>
          <DialogDescription>
            {action
              ? `Choose a new binding for "${KEYBIND_LABELS[action]}". Escape to cancel.`
              : null}
          </DialogDescription>
        </DialogHeader>
        {action ? (
          <CaptureBody onCancel={onCancel} onCapture={onCapture} />
        ) : null}
        <DialogFooter>
          <Button type="button" variant="outline" onClick={onCancel}>
            Cancel
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Inner body keyed on the dialog being open. Mounted only while the dialog
 * is visible, so the listener installs on mount and tears down on close —
 * no useEffect-driven state shuffling required.
 */
function CaptureBody({
  onCancel,
  onCapture,
}: {
  onCancel: () => void;
  onCapture: (key: string) => void;
}) {
  const [pending, setPending] = useState<string | null>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onCancel();
        return;
      }
      // Skip pure modifiers — they make no sense as bindings on their own.
      // The user has to press an actual key while holding the modifiers.
      if (
        e.key === "Shift" ||
        e.key === "Control" ||
        e.key === "Alt" ||
        e.key === "Meta"
      ) {
        return;
      }
      e.preventDefault();
      // Capture-phase + stop-propagation keeps the bubble-phase listeners
      // (the global hotkey dispatcher in particular) from also firing on
      // the same keystroke and yanking the user out of the editor mid-bind.
      e.stopPropagation();
      // Encode the full chord (modifiers + key) so users can record combos
      // like Ctrl+Shift+F as a single binding. `comboFromEvent` produces
      // the canonical `+`-joined form that the reader and global
      // dispatcher consume.
      const captured = comboFromEvent(e);
      // Showing the captured key for one frame before closing makes the
      // capture feel deliberate; calling setState inside the listener is
      // correct here (it's a user event, not a derived-state recompute).
      setPending(captured);
      onCapture(captured);
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () =>
      window.removeEventListener("keydown", onKey, { capture: true });
  }, [onCancel, onCapture]);

  return (
    <div className="grid place-items-center py-6">
      <kbd className="border-border bg-muted text-foreground rounded border px-3 py-2 font-mono text-base">
        {pending ? formatKey(pending) : "Listening…"}
      </kbd>
    </div>
  );
}
