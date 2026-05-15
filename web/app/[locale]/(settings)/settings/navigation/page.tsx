import { PageHeader } from "@/components/admin/PageHeader";
import { NavigationManager } from "@/components/sidebar-layout/NavigationManager";

export default function NavigationSettingsPage() {
  return (
    <>
      <PageHeader
        title="Sidebar"
        description="Rearrange libraries, pages, and saved views in the left sidebar. Drag to reorder, toggle to hide, add custom headers or spacers to organize sections."
      />
      <NavigationManager />
    </>
  );
}
