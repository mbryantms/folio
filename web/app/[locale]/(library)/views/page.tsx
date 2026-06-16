import { ViewsIndex } from "@/components/saved-views/ViewsIndex";

/** The unified saved-content index (A3): Filter views · Reading lists ·
 *  Collections, each with in-page create/import. `/settings/views` keeps
 *  arrangement (pins + sidebar) only; per-view detail stays at
 *  `/views/[id]`. */
export default function ViewsIndexPage() {
  return <ViewsIndex />;
}
