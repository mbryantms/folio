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
      <ThemePicker />
      <CoverStackPicker />
      <EffectsPicker />
      <AppInstallation />
    </>
  );
}
