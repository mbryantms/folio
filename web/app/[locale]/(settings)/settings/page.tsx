import { SettingsIndex } from "@/components/settings/SettingsIndex";

/** `/settings` landing — a real index of every settings section (A2/A5).
 *  Replaces the old `redirect("/settings/reading")` so Settings is a genuine
 *  destination for the breadcrumb root, the UserFooter, and deep links. */
export default function SettingsPage() {
  return <SettingsIndex />;
}
