import { apiGet } from "@/lib/api/fetch";
import type { PublicAuthConfigView } from "@/lib/api/types";

import { ForgotPasswordForm } from "./ForgotPasswordForm";

// Mirrors the sign-in page's defensive default so a transient
// /auth/config failure doesn't crash the page — we fall back to
// "recovery disabled" since the safer-by-default UX is to tell the
// user to contact the admin rather than let them try a form that
// will silently fail server-side.
const DEFAULT_CONFIG: PublicAuthConfigView = {
  auth_mode: "local",
  oidc_enabled: false,
  registration_open: true,
  password_recovery_enabled: false,
};

type SearchParams = {
  /** Set by the server's form-fallback 303 (`/forgot-password?sent=1`) so
   *  the page can render the "check your email" confirmation even when JS
   *  never ran on the submit. */
  sent?: string;
};

export default async function ForgotPasswordPage({
  searchParams,
}: {
  searchParams: Promise<SearchParams>;
}) {
  const sp = await searchParams;
  let config = DEFAULT_CONFIG;
  try {
    config = await apiGet<PublicAuthConfigView>("/auth/config");
  } catch {
    /* keep safe defaults */
  }
  return (
    <div className="bg-background flex min-h-screen items-center justify-center px-4 py-12">
      <div className="w-full max-w-sm">
        <ForgotPasswordForm
          preSubmitted={sp.sent === "1"}
          recoveryEnabled={config.password_recovery_enabled}
        />
      </div>
    </div>
  );
}
