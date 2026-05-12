import { redirect } from "next/navigation";

import { apiGet, ApiError } from "@/lib/api/fetch";
import type { PublicAuthConfigView } from "@/lib/api/types";

import { SignInClient } from "./SignInClient";
import { isSafeNextPath } from "./safe-next";

const DEFAULT_CONFIG: PublicAuthConfigView = {
  auth_mode: "local",
  oidc_enabled: false,
  registration_open: true,
};

type SearchParams = {
  next?: string;
  verified?: string;
  reset?: string;
  pending?: string;
  /** Surfaced when an OIDC callback returns a structured error. */
  error?: string;
};

export default async function SignInPage({
  searchParams,
}: {
  searchParams: Promise<SearchParams>;
}) {
  const sp = await searchParams;
  const nextRaw = sp.next ?? null;
  const next: string | null = isSafeNextPath(nextRaw) ? nextRaw : null;

  // If the user is already authenticated, bounce them to the target
  // (or "/"). Saves an extra round-trip from /sign-in → /.
  try {
    await apiGet("/auth/me");
    redirect(next ?? "/");
  } catch (e) {
    if (!(e instanceof ApiError) || e.status !== 401) {
      // Unknown failure — let the form render anyway; the form's own
      // error path will surface the issue on submit.
    }
  }

  let config = DEFAULT_CONFIG;
  try {
    config = await apiGet<PublicAuthConfigView>("/auth/config");
  } catch {
    /* fall back to local-only defaults */
  }

  const banner = sp.verified
    ? ("verified" as const)
    : sp.reset
      ? ("reset" as const)
      : sp.pending
        ? ("pending" as const)
        : sp.error
          ? ("error" as const)
          : null;

  return (
    <SignInClient
      config={config}
      next={next}
      banner={banner}
      errorMessage={sp.error ?? null}
    />
  );
}
