import { ViewsIndex } from "@/components/saved-views/ViewsIndex";

/** Settings → Views: the unified saved-content manager (A3). Browse,
 *  create/import, and arrange (pin-to-pages + sidebar) all three saved
 *  types — Filter views, Reading lists, Collections — in one tabbed page.
 *  `ViewsIndex` owns its own PageHeader. */
export default function ViewsSettingsPage() {
  return <ViewsIndex />;
}
