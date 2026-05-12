import { PageHeader } from "@/components/admin/PageHeader";
import { UserTable } from "@/components/admin/users/UserTable";

export default async function UsersPage() {
  return (
    <>
      <PageHeader
        title="Users"
        description="Promote, disable, and manage per-library access for accounts."
      />
      <UserTable />
    </>
  );
}
