import { PageHeader } from "@/components/admin/PageHeader";
import { ThemePicker } from "@/components/settings/ThemePicker";

export default function ThemeSettingsPage() {
  return (
    <>
      <PageHeader
        title="Theme"
        description="Theme, accent, and density. Changes apply instantly and sync across devices."
      />
      <ThemePicker />
    </>
  );
}
