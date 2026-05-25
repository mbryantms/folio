import { redirect } from "next/navigation";

/**
 * `/admin/audit` was a single-source view of `audit_log`, but
 * `/admin/activity` already unifies that table with scan-runs +
 * health + reading volume behind filter chips. Keeping two surfaces
 * that share a backing table cost mental-model density without
 * adding any reach.
 *
 * The route stays so bookmarks resolve, but it forwards into the
 * unified feed with the Audit chip pre-applied — same data, one
 * home.
 */
export default function AuditRedirectPage() {
  redirect("/admin/activity?kinds=audit");
}
