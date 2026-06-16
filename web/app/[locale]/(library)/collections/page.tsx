import { CollectionsIndex } from "@/components/collections/CollectionsIndex";

/** Standalone collections-only browse page — the main-UI sidebar
 *  "Collections" link lands here. The full three-type management view
 *  (with create/import + pin/sidebar arrangement) lives at
 *  `/settings/views`. */
export default function CollectionsPage() {
  return <CollectionsIndex />;
}
