import { cookies } from "next/headers";
import { redirect } from "next/navigation";

import { AdminShell } from "@/components/admin/AdminShell";
import { adminNav } from "@/components/admin/nav";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type { MeView } from "@/lib/api/types";
import { SIDEBAR_COOKIE, parseSidebarState } from "@/lib/sidebar-state";

export default async function AdminLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  let me: MeView;
  try {
    me = await apiGet<MeView>("/auth/me");
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) {
      redirect(`/sign-in`);
    }
    throw e;
  }
  if (me.role !== "admin") {
    redirect(`/`);
  }
  const defaultSidebar = parseSidebarState(
    (await cookies()).get(SIDEBAR_COOKIE)?.value,
  );
  return (
    <AdminShell
      user={me}
      sections={adminNav("")}
      title="Admin"
      homeHref={`/`}
      showScanBeacon
      defaultSidebar={defaultSidebar}
    >
      {children}
    </AdminShell>
  );
}
