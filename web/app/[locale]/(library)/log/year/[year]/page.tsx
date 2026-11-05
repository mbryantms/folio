import { notFound } from "next/navigation";

import { YearReview } from "@/components/log/year/YearReview";
import { parseYear } from "@/lib/log/year";

/** `/log/year/<year>` — "Your <year> in comics", the full-screen Year in
 *  review story (folio-flair phase 4). The log layout hides its header and
 *  tabs here; the story draws its own frame. Plan:
 *  ~/.claude/plans/folio-flair-phase4-log.md */
export default async function YearInReviewRoute({
  params,
}: {
  params: Promise<{ year: string }>;
}) {
  const year = parseYear((await params).year);
  if (year == null) notFound();
  return <YearReview year={year} />;
}
