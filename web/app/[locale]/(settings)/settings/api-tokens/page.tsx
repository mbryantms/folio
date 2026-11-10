import { PageHeader } from "@/components/admin/PageHeader";
import { AppPasswordsCard } from "@/components/settings/AppPasswordsCard";

export default function ApiTokensPage() {
  return (
    <>
      <PageHeader
        title="App passwords"
        description="Long-lived passwords for OPDS and other reader apps. They sign the app in to the catalogue, downloads, page streaming and progress sync, and don’t work on the rest of the API."
      />
      <AppPasswordsCard />
    </>
  );
}
