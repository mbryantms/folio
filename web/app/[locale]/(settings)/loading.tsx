import { SettingsRouteSkeleton } from "@/components/skeletons/SettingsRouteSkeleton";

/**
 * `(settings)` group fallback. Renders inside `AdminShell` (settings nav),
 * for every settings page — so it reads the pathname and draws that
 * page's `PageHeader` + body shape instead of one generic form.
 */
export default function SettingsLoading() {
  return <SettingsRouteSkeleton />;
}
