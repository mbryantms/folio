import { ActivityTimeline } from "@/components/activity/ActivityTimeline";

/** `/log/sessions` — the reading log's Sessions tab: every reading
 *  session, newest first, grouped by day (folio-flair phase 4). */
export default function ReadingSessionsRoute() {
  return (
    <div className="space-y-4">
      <p className="text-muted-foreground max-w-prose text-sm">
        Each time you sat down to read: when, what, and for how long.
      </p>
      <section
        aria-label="Reading sessions"
        className="bg-card rounded-xl border p-5"
      >
        <ActivityTimeline scope={{ type: "all" }} />
      </section>
    </div>
  );
}
