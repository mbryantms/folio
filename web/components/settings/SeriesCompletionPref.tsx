"use client";

import { Switch } from "@/components/ui/switch";
import { useSetCountSpecialsInCompleteness } from "@/lib/api/mutations/completion";
import { useMe } from "@/lib/api/queries";

import { SettingsSection } from "./SettingsSection";

/**
 * Account setting (remediation B9): whether annuals, one-shots, specials
 * and TPBs count toward a series' collection completeness — the card
 * dot, the series page's Collection badge and tab, and the "Collection
 * Completeness" filter. A series' ⋯ menu can override it either way.
 */
export function SeriesCompletionPref() {
  const me = useMe();
  const update = useSetCountSpecialsInCompleteness();
  if (!me.data) return null;
  const checked = me.data.count_specials_in_completeness === true;

  return (
    <SettingsSection
      title="Series completion"
      description="What a series needs on the shelf to read as complete."
    >
      <div className="flex items-center justify-between gap-6">
        <div className="space-y-0.5">
          <p
            id="count-specials-label"
            className="text-foreground text-sm font-medium"
          >
            Count annuals and specials toward series completion
          </p>
          <p id="count-specials-help" className="text-muted-foreground text-sm">
            When off, only the main numbered run is compared with the
            publisher&rsquo;s issue count. When on, annuals, one-shots, specials
            and TPBs in the series count too. Any series can override this from
            its ⋯ menu.
          </p>
        </div>
        <Switch
          checked={checked}
          onCheckedChange={(v) => update.mutate(v)}
          disabled={update.isPending}
          aria-labelledby="count-specials-label"
          aria-describedby="count-specials-help"
        />
      </div>
    </SettingsSection>
  );
}
