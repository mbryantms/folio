import { redirect } from "next/navigation";

/** /views moved to /settings/views (per-user management surface). The
 *  per-view detail at /views/[id] still lives here. */
export default function ViewsIndexRedirect() {
  redirect("/settings/views");
}
