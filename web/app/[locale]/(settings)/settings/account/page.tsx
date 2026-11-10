import { PageHeader } from "@/components/admin/PageHeader";
import {
  AccountForm,
  type EmailChangeStatus,
} from "@/components/settings/AccountForm";

/** `?email_change=` is set by the email-change confirmation link's
 *  redirect (and by the no-JS form fallback after a request). */
function parseEmailChangeStatus(
  v: string | string[] | undefined,
): EmailChangeStatus | null {
  const s = Array.isArray(v) ? v[0] : v;
  return s === "confirmed" || s === "invalid" || s === "taken" || s === "sent"
    ? s
    : null;
}

export default async function AccountSettingsPage({
  searchParams,
}: {
  searchParams: Promise<{ email_change?: string | string[] }>;
}) {
  const sp = await searchParams;
  return (
    <>
      <PageHeader
        title="Account"
        description="Display name, email, password, and a download of your data."
      />
      <AccountForm emailChange={parseEmailChangeStatus(sp.email_change)} />
    </>
  );
}
