"use client";

import { AuthConfigSkeleton } from "@/components/skeletons/AdminShapes";
import { useAdminSettings, useAuthConfig } from "@/lib/api/queries";

import { AuthConfigForm, type AuthConfigInitial } from "./AuthConfigForm";
import { TokensCard } from "./TokensCard";

export function AuthConfigEditor() {
  const settings = useAdminSettings();
  // The live config: its `auth_mode` is the effective mode, which differs
  // from the `auth.mode` row when no mode has been saved (the server then
  // derives it from whether single sign-on is configured).
  const live = useAuthConfig();

  if (settings.isLoading || live.isLoading) return <AuthConfigSkeleton />;
  if (settings.error || !settings.data) {
    return (
      <p className="text-destructive text-sm">Failed to load auth settings.</p>
    );
  }

  const asString = (k: string, fallback: string) => {
    const r = settings.data.values.find((x) => x.key === k);
    return typeof r?.value === "string" ? r.value : fallback;
  };

  return (
    <div className="space-y-4">
      <AuthConfigForm
        initial={pickAuthValues(settings.data.values, live.data?.auth_mode)}
      />
      <TokensCard
        initial={{
          access_ttl: asString("auth.jwt.access_ttl", "1h"),
          refresh_ttl: asString("auth.jwt.refresh_ttl", "30d"),
          session_max_age: asString("auth.session.max_age", "90d"),
        }}
      />
    </div>
  );
}

function isMode(v: unknown): v is AuthConfigInitial["mode"] {
  return v === "local" || v === "oidc" || v === "both";
}

function pickAuthValues(
  rows: { key: string; value: unknown; is_secret: boolean }[],
  effectiveMode: string | undefined,
): AuthConfigInitial {
  const asString = (k: string) => {
    const r = rows.find((x) => x.key === k);
    return typeof r?.value === "string" ? r.value : "";
  };
  const asBool = (k: string, fallback: boolean) => {
    const r = rows.find((x) => x.key === k);
    return typeof r?.value === "boolean" ? r.value : fallback;
  };
  const saved = asString("auth.mode");
  const effective = isMode(effectiveMode) ? effectiveMode : "local";
  return {
    mode: isMode(saved) ? saved : effective,
    mode_saved: isMode(saved),
    registration_open: asBool("auth.local.registration_open", true),
    oidc_issuer: asString("auth.oidc.issuer"),
    oidc_client_id: asString("auth.oidc.client_id"),
    oidc_client_secret_set: rows.some(
      (r) =>
        r.key === "auth.oidc.client_secret" &&
        r.is_secret &&
        r.value === "<set>",
    ),
    oidc_trust_unverified_email: asBool(
      "auth.oidc.trust_unverified_email",
      false,
    ),
    oidc_link_local_by_verified_email: asBool(
      "auth.oidc.link_local_by_verified_email",
      false,
    ),
    oidc_auto_redirect: asBool("auth.oidc.auto_redirect", false),
    oidc_grant_all_libraries: asBool("auth.oidc.grant_all_libraries", false),
  };
}
