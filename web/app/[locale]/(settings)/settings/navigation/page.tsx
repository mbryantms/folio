import { PageHeader } from "@/components/admin/PageHeader";
import { NavigationManager } from "@/components/sidebar-layout/NavigationManager";

export default function NavigationSettingsPage() {
  return (
    <>
      <PageHeader
        title="Navigation"
        description="Customize what lives on your home page and in the left sidebar. Drag to reorder; toggle to hide. Resets here don't affect the underlying views — manage those from Saved views."
      />
      <NavigationManager />
    </>
  );
}
