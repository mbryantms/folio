import { redirect } from "next/navigation";

/** Collections moved into the unified `/views` index (A3) as the
 *  `#collections` section. Redirect keeps old links + the sidebar entry
 *  working; the fragment lands the user on the Collections section. */
export default function CollectionsIndexRedirectPage() {
  redirect("/views#collections");
}
