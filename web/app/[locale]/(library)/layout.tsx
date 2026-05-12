import { cookies } from "next/headers";
import { redirect } from "next/navigation";

import { MainShell } from "@/components/library/MainShell";
import { mainNav } from "@/components/library/main-nav";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type { LibraryView, MeView, SavedViewListView } from "@/lib/api/types";
import { SIDEBAR_COOKIE, parseSidebarState } from "@/lib/sidebar-state";

/**
 * Wraps the library / series / issue routes with the persistent left-rail
 * shell. Reader (`/read/...`) is intentionally outside this group so it
 * stays full-screen.
 *
 * Per-library entries are populated server-side from `/libraries`, which
 * already enforces ACL — so the sidebar shows only what the calling user
 * can actually see.
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

  let libraries: LibraryView[] = [];
  try {
    libraries = await apiGet<LibraryView[]>("/libraries");
  } catch {
    /* empty — sidebar still renders, just without per-library entries */
  }

  // Saved views the user wants in their left rail. Best-effort: a
  // failure here just means an empty Saved-views section.
  let sidebarViews: SavedViewListView["items"] = [];
  try {
    const list = await apiGet<SavedViewListView>(
      "/me/saved-views?show_in_sidebar=true",
    );
    sidebarViews = list.items;
  } catch {
    /* empty */
  }

  const defaultSidebar = parseSidebarState(
    (await cookies()).get(SIDEBAR_COOKIE)?.value,
  );

  return (
    <MainShell
      user={me}
      sections={mainNav("", libraries, sidebarViews)}
      homeHref={`/`}
      defaultSidebar={defaultSidebar}
      showMarkerCount={me.show_marker_count}
    >
      {children}
    </MainShell>
  );
}
