"use client";

import { Library } from "lucide-react";

import {
  CoverStack,
  useCoverStackStyle,
} from "@/components/cover-stack/CoverStack";
import { useUpdatePreferences } from "@/lib/api/mutations";
import { useViewCovers } from "@/lib/api/queries";
import {
  COVER_STACK_STYLES,
  VARIANTS,
  type CoverStackStyle,
} from "@/lib/cover-stack/variants";

import { SettingsSection } from "./SettingsSection";

/** Settings → Theme: how reading-list and collection cards fan their
 *  covers (`users.cover_stack_style`). Each option is a live preview —
 *  hover or focus it to see the fan — drawn from the user's own fullest
 *  list, falling back to placeholder tiles before they have one. */
export function CoverStackPicker() {
  const current = useCoverStackStyle();
  const update = useUpdatePreferences({ silent: true });
  const covers = useViewCovers();

  // The list with the most covers makes the best sample.
  let sample: readonly string[] = [];
  for (const urls of covers.data?.values() ?? []) {
    if (urls.length > sample.length) sample = urls;
  }

  function pick(next: CoverStackStyle) {
    if (next !== current) update.mutate({ cover_stack_style: next });
  }

  return (
    <SettingsSection
      title="Cover stacks"
      description="How reading-list and collection cards fan out their covers when you hover or focus them."
    >
      <div
        role="radiogroup"
        aria-label="Cover stack style"
        className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5"
      >
        {COVER_STACK_STYLES.map((style) => {
          const active = style === current;
          const v = VARIANTS[style];
          return (
            <button
              key={style}
              type="button"
              role="radio"
              aria-checked={active}
              disabled={update.isPending}
              onClick={() => pick(style)}
              className={`cover-stack-host focus-visible:ring-ring flex flex-col items-start gap-3 rounded-lg border p-3 text-left transition-colors focus-visible:ring-2 focus-visible:ring-offset-1 focus-visible:outline-none disabled:opacity-60 ${
                active
                  ? "border-primary bg-primary/10"
                  : "border-input hover:bg-accent/40"
              }`}
            >
              <CoverStack
                urls={sample}
                style={style}
                Icon={Library}
                spill={false}
                className="w-full max-w-32"
              />
              <span className="space-y-0.5">
                <span className="block text-sm font-medium">{v.label}</span>
                <span className="text-muted-foreground block text-xs">
                  {v.description}
                </span>
              </span>
            </button>
          );
        })}
      </div>
    </SettingsSection>
  );
}
