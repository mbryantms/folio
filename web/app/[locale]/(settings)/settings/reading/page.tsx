import { PageHeader } from "@/components/admin/PageHeader";
import { ReadingPrefs } from "@/components/settings/ReadingPrefs";

export default function ReadingSettingsPage() {
  return (
    <>
      <PageHeader
        title="Reading"
        description="Defaults applied when you open a series for the first time. Per-series overrides and ComicInfo metadata still take precedence."
      />
      <ReadingPrefs />
    </>
  );
}
