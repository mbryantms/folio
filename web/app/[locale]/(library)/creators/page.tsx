import { CreatorsIndex } from "@/components/library/CreatorsIndex";
import { parseStartsWithParam } from "@/components/library/library-grid-filters";

/** Alphabetical creator browse index (audit A11). The global-search
 *  `/people` endpoint can only answer a query — it returns nothing for
 *  an empty `q` — so it can't back a browse-all directory. This page
 *  hits the dedicated cursor-paginated `GET /creators` instead, which
 *  walks every visible creator without silently truncating. Each card
 *  links to the existing `/creators/<slug>` detail page. Client
 *  component so the infinite-scroll cursor state lives next to the
 *  TanStack Query cache; the `?starts_with=` jump-rail bucket is parsed
 *  here so a deep-link hydrates with the letter pre-applied. */
export default async function CreatorsIndexPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | undefined>>;
}) {
  const params = await searchParams;
  return (
    <CreatorsIndex
      initialStartsWith={parseStartsWithParam(params.starts_with) ?? null}
    />
  );
}
