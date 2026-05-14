import { cookies } from "next/headers";
import { redirect } from "next/navigation";

import { MainShell } from "@/components/library/MainShell";
import { mainNav } from "@/components/library/main-nav";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type { MeView, SidebarLayoutView } from "@/lib/api/types";
import { SIDEBAR_COOKIE, parseSidebarState } from "@/lib/sidebar-state";

/**
 * Wraps the library / series / issue routes with the persistent left-rail
 * shell. Reader (`/read/...`) is intentionally outside this group so it
 * stays full-screen.
 *
 * Sidebar contents come from `/me/sidebar-layout` — a single endpoint
 * that resolves built-ins, ACL-filtered libraries, and saved-view
 * sidebar entries in one shot, applying the user's drag-reorder and
 * hide-toggle overrides from the navigation customization settings.
 */
export default async function LibraryLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  let me: MeView | null = null;
  try {
    me = await apiGet<MeView>("/auth/me");
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) {
      // Anonymous fall-through. The home page handles unauthenticated state
      // with its own sign-in CTA; we don't redirect here so the public-facing
      // bits (404, sign-in) keep working.
      me = null;
    } else {
      throw e;
    }
  }

  if (!me) {
    redirect(`/sign-in`);
  }

  // Best-effort: a failure here just degrades to an empty sidebar; the
  // top-bar still renders so the user can navigate.
  let layout: SidebarLayoutView = { entries: [] };
  try {
    layout = await apiGet<SidebarLayoutView>("/me/sidebar-layout");
  } catch {
    /* empty */
  }

  const defaultSidebar = parseSidebarState(
    (await cookies()).get(SIDEBAR_COOKIE)?.value,
  );

  return (
    <MainShell
      user={me}
      sections={mainNav("", layout)}
      homeHref={`/`}
      defaultSidebar={defaultSidebar}
      showMarkerCount={me.show_marker_count}
    >
      {children}
    </MainShell>
  );
}
