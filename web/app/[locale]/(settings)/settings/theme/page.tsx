import { AppInstallation } from "@/components/settings/AppInstallation";
import { PageHeader } from "@/components/admin/PageHeader";
import { CoverStackPicker } from "@/components/settings/CoverStackPicker";
import { EffectsPicker } from "@/components/settings/EffectsPicker";
import { ThemePicker } from "@/components/settings/ThemePicker";

export default function ThemeSettingsPage() {
  return (
    <>
      <PageHeader
        title="Theme"
        description="Theme, accent, density, cover stacks, and effects. Changes apply instantly and sync across devices."
      />
      {/* One rhythm for every section: ThemePicker's own sections are
          space-y-6, so the pickers after it match that gap. */}
      <div className="space-y-6">
        <ThemePicker />
        <CoverStackPicker />
        <EffectsPicker />
        <AppInstallation />
      </div>
    </>
  );
}
