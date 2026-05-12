import { PageHeader } from "@/components/admin/PageHeader";
import { AppPasswordsCard } from "@/components/settings/AppPasswordsCard";

export default function ApiTokensPage() {
  return (
    <>
      <PageHeader
        title="App passwords"
        description="Long-lived Bearer tokens for OPDS readers, scripts, and other clients that can’t use the cookie session."
      />
      <AppPasswordsCard />
    </>
  );
}
