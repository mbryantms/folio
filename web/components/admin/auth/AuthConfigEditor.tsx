"use client";

import { Skeleton } from "@/components/ui/skeleton";
import { useAdminSettings } from "@/lib/api/queries";

import { AuthConfigForm, type AuthConfigInitial } from "./AuthConfigForm";
import { TokensCard } from "./TokensCard";

export function AuthConfigEditor() {
  const settings = useAdminSettings();

  if (settings.isLoading) return <Skeleton className="h-96 w-full" />;
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
      <AuthConfigForm initial={pickAuthValues(settings.data.values)} />
      <TokensCard
        initial={{
          access_ttl: asString("auth.jwt.access_ttl", "24h"),
          refresh_ttl: asString("auth.jwt.refresh_ttl", "30d"),
        }}
      />
    </div>
  );
}

function pickAuthValues(
  rows: { key: string; value: unknown; is_secret: boolean }[],
): AuthConfigInitial {
  const asString = (k: string) => {
    const r = rows.find((x) => x.key === k);
    return typeof r?.value === "string" ? r.value : "";
  };
  const asBool = (k: string, fallback: boolean) => {
    const r = rows.find((x) => x.key === k);
    return typeof r?.value === "boolean" ? r.value : fallback;
  };
  const mode = asString("auth.mode");
  return {
    mode: mode === "local" || mode === "oidc" || mode === "both" ? mode : "both",
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
  };
}
