import { redirect } from "next/navigation";

/** The sidebar-arrangement surface moved to `/settings/sidebar` so the
 *  settings → Library trio reads Views · Pages · Sidebar with matching
 *  routes. Old bookmarks / inbound links land on the new route. */
export default function NavigationSettingsRedirect() {
  redirect("/settings/sidebar");
}
