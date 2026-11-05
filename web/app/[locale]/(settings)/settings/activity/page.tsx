import { redirect } from "next/navigation";

/** Old home of the privacy controls ("Reading history & privacy");
 *  bookmarks and recent-visit entries still point here. */
export default function ActivitySettingsRedirect() {
  redirect("/settings/privacy");
}
