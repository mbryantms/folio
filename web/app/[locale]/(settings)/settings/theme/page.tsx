import { AppInstallation } from "@/components/settings/AppInstallation";
import { PageHeader } from "@/components/admin/PageHeader";
import { CoverStackPicker } from "@/components/settings/CoverStackPicker";
import { ThemePicker } from "@/components/settings/ThemePicker";

export default function ThemeSettingsPage() {
  return (
    <>
      <PageHeader
        title="Theme"
        description="Theme, accent, density, and cover stacks. Changes apply instantly and sync across devices."
      />
      <ThemePicker />
      <CoverStackPicker />
      <AppInstallation />
    </>
  );
}
