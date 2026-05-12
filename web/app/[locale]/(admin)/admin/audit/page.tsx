import { PageHeader } from "@/components/admin/PageHeader";
import { AuditTable } from "@/components/admin/users/AuditTable";

export default function AuditPage() {
  return (
    <>
      <PageHeader
        title="Audit log"
        description="Searchable trail of admin and authentication events."
      />
      <AuditTable />
    </>
  );
}
