import { ReadingLogPage } from "@/components/log/ReadingLogPage";

/** `/log` — reverse-chronological reading-activity feed plus a
 *  right-rail of summary widgets (stats hero, year heatmap, top
 *  creators). M2 ships a hard-coded default layout; M3-M5 will turn
 *  the right rail into a customizable widget grid. Spec at
 *  ~/.claude/plans/reading-log.md. */
export default function ReadingLogRoute() {
  return <ReadingLogPage />;
}
