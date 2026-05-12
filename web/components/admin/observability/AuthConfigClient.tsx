"use client";

import { Info, Lock, ShieldCheck } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { useAuthConfig } from "@/lib/api/queries";
import type { AuthConfigView } from "@/lib/api/types";
import { cn } from "@/lib/utils";

export function AuthConfigClient() {
  const cfg = useAuthConfig();
  if (cfg.isLoading || !cfg.data) return <Skeleton className="h-64 w-full" />;
  if (cfg.error) {
    return (
      <p className="text-destructive text-sm">Failed to load auth config.</p>
    );
  }
  const data = cfg.data;

  return (
    <div className="space-y-4">
      <div className="flex items-start gap-2 rounded-md border border-amber-500/30 bg-amber-500/5 p-3 text-sm text-amber-200/80">
        <Info className="mt-0.5 h-4 w-4 shrink-0" />
        <span>
          Auth configuration is loaded from environment variables on boot.
          Editing these values requires updating your compose / env file and
          restarting the server.
        </span>
      </div>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <ModeCard data={data} />
        <LocalCard data={data} />
        {data.oidc.configured ? <OidcCard data={data} /> : <OidcDisabledCard />}
      </div>
    </div>
  );
}

function ModeCard({ data }: { data: AuthConfigView }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Mode
        </CardTitle>
      </CardHeader>
      <CardContent>
        <p className="text-foreground flex items-center gap-2 text-base font-semibold">
          <ShieldCheck className="text-primary h-4 w-4" />
          {labelForMode(data.auth_mode)}
        </p>
        <p className="text-muted-foreground mt-1 text-xs">
          Set via <code className="font-mono">COMIC_AUTH_MODE</code>.
        </p>
      </CardContent>
    </Card>
  );
}

function LocalCard({ data }: { data: AuthConfigView }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          Local auth
        </CardTitle>
      </CardHeader>
      <CardContent>
        <ul className="space-y-2 text-sm">
          <Row label="Enabled" status={data.local.enabled} />
          <Row
            label="Self-serve registration"
            status={data.local.registration_open}
          />
          <Row label="SMTP wired" status={data.local.smtp_configured} />
        </ul>
        <p className="text-muted-foreground mt-3 text-xs">
          When SMTP isn&rsquo;t configured, new local accounts skip email
          verification (first user becomes admin).
        </p>
      </CardContent>
    </Card>
  );
}

function OidcCard({ data }: { data: AuthConfigView }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          OIDC
        </CardTitle>
      </CardHeader>
      <CardContent>
        <dl className="space-y-2 text-sm">
          <KeyRow label="Issuer" value={data.oidc.issuer ?? "—"} mono />
          <KeyRow label="Client ID" value={data.oidc.client_id ?? "—"} mono />
          <Row
            label="Trust unverified email"
            status={data.oidc.trust_unverified_email}
            tone="warn"
          />
        </dl>
        <p className="text-muted-foreground mt-3 text-xs">
          Client secret is never returned by this endpoint.
        </p>
      </CardContent>
    </Card>
  );
}

function OidcDisabledCard() {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
          OIDC
        </CardTitle>
      </CardHeader>
      <CardContent className="text-muted-foreground text-sm">
        <p className="flex items-center gap-2">
          <Lock className="h-4 w-4" />
          Not configured. Set{" "}
          <code className="font-mono">COMIC_OIDC_ISSUER</code> + client
          credentials to enable single sign-on.
        </p>
      </CardContent>
    </Card>
  );
}

function Row({
  label,
  status,
  tone,
}: {
  label: string;
  status: boolean;
  tone?: "warn";
}) {
  const goodTone =
    tone === "warn"
      ? "border-amber-500/40 text-amber-300"
      : "border-emerald-500/40 text-emerald-400";
  const badTone =
    tone === "warn"
      ? "border-emerald-500/40 text-emerald-400"
      : "border-border text-muted-foreground";
  return (
    <li className="flex items-center justify-between">
      <span className="text-muted-foreground">{label}</span>
      <Badge
        variant="outline"
        className={cn("font-mono text-xs", status ? goodTone : badTone)}
      >
        {status ? "Yes" : "No"}
      </Badge>
    </li>
  );
}

function KeyRow({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="flex items-baseline justify-between gap-3">
      <dt className="text-muted-foreground shrink-0">{label}</dt>
      <dd
        className={cn(
          "text-foreground min-w-0 truncate",
          mono && "font-mono tabular-nums",
        )}
      >
        {value}
      </dd>
    </div>
  );
}

function labelForMode(mode: string): string {
  switch (mode) {
    case "local":
      return "Local accounts only";
    case "oidc":
      return "OIDC SSO only";
    case "both":
      return "Local + OIDC";
    default:
      return mode;
  }
}
