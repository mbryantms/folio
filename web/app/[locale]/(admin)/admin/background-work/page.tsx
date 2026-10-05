import { PageHeader } from "@/components/admin/PageHeader";
import { BackgroundWorkClient } from "@/components/admin/background/BackgroundWorkClient";

export default function BackgroundWorkPage() {
  return (
    <>
      <PageHeader
        title="Background work"
        description="Everything the server is doing right now, in one place: scans, thumbnail and cover generation, content hashing, metadata and archive jobs — per library and per queue."
      />
      <BackgroundWorkClient />
    </>
  );
}
