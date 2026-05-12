import { Suspense } from "react";

import { PageHeader } from "@/components/admin/PageHeader";
import { QuickApplyPrefill } from "@/components/saved-views/QuickApplyPrefill";
import { SavedViewsManager } from "@/components/saved-views/SavedViewsManager";

export default function SavedViewsSettingsPage() {
  return (
    <>
      <PageHeader
        title="Saved views"
        description="Filter views and CBL reading lists. Drag to reorder pinned views; pin/unpin to control what shows up on the home page."
      />
      <SavedViewsManager />
      {/* QuickApplyPrefill reads `?quick_field=&quick_value=` to open
          the New filter view dialog pre-filled — wired by chip-list
          links on the series detail page. Suspense is required since
          `useSearchParams` flips it on. */}
      <Suspense fallback={null}>
        <QuickApplyPrefill />
      </Suspense>
    </>
  );
}
