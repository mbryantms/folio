import { PageHeader } from "@/components/admin/PageHeader";
import { NavigationManager } from "@/components/sidebar-layout/NavigationManager";

export default function SidebarSettingsPage() {
  return (
    <>
      <PageHeader
        title="Sidebar"
        description="Reorder the sidebar, hide items, rename them, and add headers or spacers. Most of this can also be done in place: choose Edit sidebar at the bottom of the sidebar. Use this page to add saved views that are not in the sidebar yet, or to reset it."
      />
      <NavigationManager />
    </>
  );
}
