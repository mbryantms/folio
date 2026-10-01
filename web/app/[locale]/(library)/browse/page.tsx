import { BrowseTabs } from "@/components/library/BrowseTabs";
import { parseBrowseTab } from "@/components/library/browse-tabs";
import { parseStartsWithParam } from "@/components/library/library-grid-filters";

/** `/browse` — the single "Browse" sidebar destination (WP-5.5): a tabbed
 *  index over characters / teams / story arcs / publishers / creators.
 *  `?tab=` picks the tab; `?starts_with=` seeds its A–Z rail. */
export default async function BrowsePage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | undefined>>;
}) {
  const sp = await searchParams;
  return (
    <BrowseTabs
      initialTab={parseBrowseTab(sp.tab)}
      initialStartsWith={parseStartsWithParam(sp.starts_with) ?? null}
    />
  );
}
