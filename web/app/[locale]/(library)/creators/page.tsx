import { CreatorsIndex } from "@/components/library/CreatorsIndex";

/** Alphabetical creator browse index (audit A11). The global-search
 *  `/people` endpoint can only answer a query — it returns nothing for
 *  an empty `q` — so it can't back a browse-all directory. This page
 *  hits the dedicated cursor-paginated `GET /creators` instead, which
 *  walks every visible creator without silently truncating. Each card
 *  links to the existing `/creators/<slug>` detail page. Client
 *  component so the infinite-scroll cursor state lives next to the
 *  TanStack Query cache. */
export default function CreatorsIndexPage() {
  return <CreatorsIndex />;
}
