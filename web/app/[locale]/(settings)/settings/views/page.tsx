import { Suspense } from "react";

import { PageHeader } from "@/components/admin/PageHeader";
import { QuickApplyPrefill } from "@/components/saved-views/QuickApplyPrefill";
import { SavedViewsManager } from "@/components/saved-views/SavedViewsManager";

export default function SavedViewsSettingsPage() {
  return (
    <>
      <PageHeader
        title="Saved views"
        description="Create, edit, and delete filter views and CBL reading lists. Where they appear — home rails and sidebar — is managed from Navigation."
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
