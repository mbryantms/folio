import { PageHeader } from "@/components/admin/PageHeader";
import { EmailAdminClient } from "@/components/admin/email/EmailAdminClient";

export default function EmailAdminPage() {
  return (
    <>
      <PageHeader
        title="Email"
        description="Configure SMTP delivery for recovery flows (verify-email, password reset). Changes take effect on save without restarting."
      />
      <EmailAdminClient />
    </>
  );
}
