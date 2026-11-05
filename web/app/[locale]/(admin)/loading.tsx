import { AdminRouteSkeleton } from "@/components/skeletons/AdminRouteSkeleton";

/**
 * `(admin)` group fallback. Renders inside `AdminShell` while an admin
 * route's server components resolve. Admin pages differ too much for one
 * generic shape, so the fallback reads the pathname and draws that page's
 * `PageHeader` stand-in plus the same body skeleton its client component
 * shows while loading (`components/skeletons/AdminShapes`) — the route →
 * client handoff then holds still instead of morphing.
 */
export default function AdminLoading() {
  return <AdminRouteSkeleton />;
}
