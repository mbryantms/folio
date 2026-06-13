import { PageHeader } from "@/components/admin/PageHeader";
import { AdminMetadataTabs } from "@/components/admin/metadata/AdminMetadataTabs";

export default function AdminMetadataPage() {
  return (
    <>
      <PageHeader
        title="Metadata"
        description="ComicVine + Metron provider health, auto-synced series, and per-run history. Operational settings live in the Settings tab below."
      />
      <AdminMetadataTabs />
    </>
  );
}
