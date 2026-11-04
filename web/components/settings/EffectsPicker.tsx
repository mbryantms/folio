"use client";

import { useQueryClient } from "@tanstack/react-query";

import { Switch } from "@/components/ui/switch";
import { useUpdatePreferences } from "@/lib/api/mutations";
import { queryKeys, useMe } from "@/lib/api/queries";
import type { MeView } from "@/lib/api/types";
import {
  EFFECT_GROUPS,
  EFFECTS,
  effectSetting,
  useReducedMotion,
  type VisualEffect,
} from "@/lib/visual-effects";

import { SettingsSection } from "./SettingsSection";

/** Settings → Theme: on/off switches for the optional visual effects
 *  (`users.effects`). Each switch saves on its own; the change shows at once
 *  because the cached `me` is updated before the request returns. */
export function EffectsPicker() {
  const me = useMe();
  const qc = useQueryClient();
  const update = useUpdatePreferences({ silent: true, toastId: "effects" });
  const reduced = useReducedMotion();
  if (!me.data) return null;
  const stored = me.data.effects;

  function toggle(key: VisualEffect, next: boolean) {
    const prev = qc.getQueryData<MeView>(queryKeys.me);
    if (prev) {
      qc.setQueryData<MeView>(queryKeys.me, {
        ...prev,
        effects: { ...prev.effects, [key]: next },
      });
    }
    update.mutate(
      { effects: { [key]: next } },
      {
        onError: () => {
          if (prev) qc.setQueryData(queryKeys.me, prev);
        },
      },
    );
  }

  return (
    <SettingsSection
      title="Effects"
      description="Optional touches. Turn on the ones you like; each follows your account to every device."
    >
      <div className="space-y-6">
        {reduced && (
          <p className="text-muted-foreground bg-muted/50 rounded-md px-3 py-2 text-sm">
            Reduced motion is on, so effects marked{" "}
            <span className="text-foreground font-medium">Motion</span> stay off
            until you change the Motion setting above.
          </p>
        )}
        {EFFECT_GROUPS.map((group) => (
          <fieldset key={group.id} className="space-y-3">
            <legend className="text-muted-foreground mb-2 text-xs font-medium tracking-wide uppercase">
              {group.label}
            </legend>
            {EFFECTS.filter((e) => e.group === group.id).map((e) => {
              const id = `effect-${e.key}`;
              const checked = effectSetting(stored, e.key);
              return (
                <div
                  key={e.key}
                  className="flex items-center justify-between gap-6"
                >
                  <div className="min-w-0 space-y-0.5">
                    <p
                      id={`${id}-label`}
                      className="text-foreground flex items-center gap-2 text-sm font-medium"
                    >
                      {e.label}
                      {e.motion && (
                        <span className="text-muted-foreground rounded border px-1.5 py-px text-[10px] font-medium tracking-wide uppercase">
                          Motion
                        </span>
                      )}
                    </p>
                    <p
                      id={`${id}-help`}
                      className="text-muted-foreground text-sm"
                    >
                      {e.description}
                    </p>
                  </div>
                  <Switch
                    id={id}
                    checked={checked}
                    onCheckedChange={(v) => toggle(e.key, v)}
                    aria-labelledby={`${id}-label`}
                    aria-describedby={`${id}-help`}
                  />
                </div>
              );
            })}
          </fieldset>
        ))}
      </div>
    </SettingsSection>
  );
}
