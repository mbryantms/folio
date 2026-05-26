import { PageHeader } from "@/components/admin/PageHeader";
import { AdminMetadataTabs } from "@/components/admin/metadata/AdminMetadataTabs";

export default function AdminMetadataPage() {
  return (
    <>
      <PageHeader
        title="Metadata"
        description="ComicVine + Metron provider health, review queue for medium/low matches, and per-run history. Settings live under /admin/settings (filter: metadata.*)."
      />
      <AdminMetadataTabs />
    </>
  );
}
