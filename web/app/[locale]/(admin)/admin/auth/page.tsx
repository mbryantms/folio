import { PageHeader } from "@/components/admin/PageHeader";
import { AuthConfigClient } from "@/components/admin/observability/AuthConfigClient";

export default function AuthConfigPage() {
  return (
    <>
      <PageHeader
        title="Auth config"
        description="Read-only view of OIDC + local-auth configuration. Edit by setting environment variables and restarting."
      />
      <AuthConfigClient />
    </>
  );
}
