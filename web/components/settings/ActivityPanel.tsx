import { ArrowRight, BarChart3 } from "lucide-react";
import Link from "next/link";

import { PrivacyControls } from "./PrivacyControls";
import { SettingsSection } from "./SettingsSection";

/**
 * Settings → Reading history & privacy. The stats deep-dive moved to the
 * reading log's Stats tab (folio-flair phase 4); this page keeps only the
 * capture / aggregate switches and history deletion, plus a pointer to
 * where the stats went.
 */
export function ActivityPanel() {
  return (
    <div className="space-y-8">
      <Link
        href="/log/stats"
        className="group border-border bg-card hover:border-primary/50 focus-visible:ring-ring flex items-center gap-4 rounded-xl border p-5 transition-colors focus-visible:ring-2 focus-visible:outline-none"
      >
        <span
          aria-hidden="true"
          className="bg-primary/10 text-primary grid size-10 shrink-0 place-items-center rounded-lg"
        >
          <BarChart3 className="size-5" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="text-foreground block font-semibold">
            Your reading stats now live in the Reading log
          </span>
          <span className="text-muted-foreground block text-sm">
            Totals, weekly rhythm, pace, rankings and rereads are on the
            log&apos;s Stats tab.
          </span>
        </span>
        <ArrowRight
          aria-hidden="true"
          className="text-muted-foreground group-hover:text-primary size-5 shrink-0 transition-transform group-hover:translate-x-0.5 motion-reduce:transition-none"
        />
      </Link>

      <SettingsSection
        title="Privacy"
        description="Control whether new sessions are captured, whether your activity counts in server-wide aggregates, and reset your history."
      >
        <PrivacyControls />
      </SettingsSection>
    </div>
  );
}
