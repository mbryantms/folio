import { PageHeader } from "@/components/admin/PageHeader";
import { AuthConfigEditor } from "@/components/admin/auth/AuthConfigEditor";

export default function AuthConfigPage() {
  return (
    <>
      <PageHeader
        title="Auth config"
        description="Auth mode, local-registration, and OIDC. Changes take effect on save without restarting; the OIDC discovery cache is evicted automatically."
      />
      <AuthConfigEditor />
    </>
  );
}
