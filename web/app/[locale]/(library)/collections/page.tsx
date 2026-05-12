import { CollectionsIndex } from "@/components/collections/CollectionsIndex";

/** Index of the user's collections (kind='collection' saved views).
 *  Mirrors `/settings/views` in spirit but scoped to manual lists; the
 *  actual rendering is a client component so the `useCollections`
 *  cache + create dialog land without an extra page-level mount. */
export default function CollectionsIndexPage() {
  return <CollectionsIndex />;
}
