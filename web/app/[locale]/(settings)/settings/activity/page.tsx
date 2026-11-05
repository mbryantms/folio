import { PageHeader } from "@/components/admin/PageHeader";
import { ActivityPanel } from "@/components/settings/ActivityPanel";

export default async function ActivityPage() {
  return (
    <>
      <PageHeader
        title="Reading history & privacy"
        description="Choose whether your reading sessions are recorded and shared in server-wide stats, or delete your history."
      />
      <ActivityPanel />
    </>
  );
}
