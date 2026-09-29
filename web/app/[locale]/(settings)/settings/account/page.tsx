import { PageHeader } from "@/components/admin/PageHeader";
import { AccountForm } from "@/components/settings/AccountForm";

export default function AccountSettingsPage() {
  return (
    <>
      <PageHeader
        title="Account"
        description="Display name, email, password, and a download of your data."
      />
      <AccountForm />
    </>
  );
}
