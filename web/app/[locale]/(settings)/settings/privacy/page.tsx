import { PageHeader } from "@/components/admin/PageHeader";
import { PrivacyControls } from "@/components/settings/PrivacyControls";
import { SettingsSection } from "@/components/settings/SettingsSection";

export default function PrivacySettingsPage() {
  return (
    <>
      <PageHeader
        title="Privacy"
        description="Choose whether your reading sessions are recorded and counted in server-wide stats, or delete your history."
      />
      <SettingsSection
        title="Reading activity"
        description="Changes apply immediately and follow your account to every device."
      >
        <PrivacyControls />
      </SettingsSection>
    </>
  );
}
