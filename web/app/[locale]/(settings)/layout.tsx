import { cookies } from "next/headers";
import { redirect } from "next/navigation";

import { AdminShell } from "@/components/admin/AdminShell";
import { settingsNav } from "@/components/admin/nav";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type { MeView } from "@/lib/api/types";
import { SIDEBAR_COOKIE, parseSidebarState } from "@/lib/sidebar-state";

export default async function SettingsLayout({
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
  const sections = settingsNav("");
  if (me.role === "admin") {
    sections.push({
      label: "Admin",
      items: [
        {
          href: `/admin`,
          label: "Admin console",
          icon: "Cog",
        },
      ],
    });
  }
  const defaultSidebar = parseSidebarState(
    (await cookies()).get(SIDEBAR_COOKIE)?.value,
  );
  return (
    <AdminShell
      user={me}
      sections={sections}
      title="Settings"
      homeHref={`/`}
      defaultSidebar={defaultSidebar}
    >
      {children}
    </AdminShell>
  );
}
