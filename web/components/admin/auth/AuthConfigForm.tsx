"use client";

import { useState } from "react";
import { AlertTriangle, Search, ShieldCheck } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { SegmentedControl } from "@/components/settings/SegmentedControl";
import { Switch } from "@/components/ui/switch";
import {
  useProbeOidcDiscovery,
  useUpdateSettings,
} from "@/lib/api/mutations";
import type { OidcDiscoverResp } from "@/lib/api/types";

export type AuthConfigInitial = {
  mode: "local" | "oidc" | "both";
  registration_open: boolean;
  oidc_issuer: string;
  oidc_client_id: string;
  /** True when a `auth.oidc.client_secret` row exists in the DB. The
   *  plaintext is never returned; the form shows a placeholder and treats
   *  an empty input as "no change." */
  oidc_client_secret_set: boolean;
  oidc_trust_unverified_email: boolean;
};

type FormState = AuthConfigInitial & { oidc_client_secret_input: string };

export function AuthConfigForm({ initial }: { initial: AuthConfigInitial }) {
  const [state, setState] = useState<FormState>({
    ...initial,
    oidc_client_secret_input: "",
  });
  const [discoverResult, setDiscoverResult] = useState<
    | { kind: "idle" }
    | { kind: "ok"; data: OidcDiscoverResp }
    | { kind: "err"; message: string }
  >({ kind: "idle" });

  const update = useUpdateSettings();
  const probe = useProbeOidcDiscovery();

  const oidcEnabled = state.mode === "oidc" || state.mode === "both";
  const localEnabled = state.mode === "local" || state.mode === "both";

  const issuerMissing = oidcEnabled && !state.oidc_issuer.trim();
  const clientIdMissing = oidcEnabled && !state.oidc_client_id.trim();
  const secretMissing =
    oidcEnabled &&
    !state.oidc_client_secret_set &&
    !state.oidc_client_secret_input;
  const oidcIncomplete = issuerMissing || clientIdMissing || secretMissing;
  const dirty =
    state.mode !== initial.mode ||
    state.registration_open !== initial.registration_open ||
    state.oidc_issuer.trim() !== initial.oidc_issuer.trim() ||
    state.oidc_client_id.trim() !== initial.oidc_client_id.trim() ||
    state.oidc_client_secret_input !== "" ||
    state.oidc_trust_unverified_email !== initial.oidc_trust_unverified_email;

  async function onSave() {
    // `disabled={!dirty}` on the submit button makes the no-op path
    // unreachable, so we can build the patch without short-circuiting.
    const patch: Record<string, unknown> = {};

    if (state.mode !== initial.mode) patch["auth.mode"] = state.mode;
    if (state.registration_open !== initial.registration_open) {
      patch["auth.local.registration_open"] = state.registration_open;
    }
    if (state.oidc_issuer.trim() !== initial.oidc_issuer.trim()) {
      patch["auth.oidc.issuer"] =
        state.oidc_issuer.trim() === "" ? null : state.oidc_issuer.trim();
    }
    if (state.oidc_client_id.trim() !== initial.oidc_client_id.trim()) {
      patch["auth.oidc.client_id"] =
        state.oidc_client_id.trim() === "" ? null : state.oidc_client_id.trim();
    }
    if (state.oidc_client_secret_input !== "") {
      patch["auth.oidc.client_secret"] = state.oidc_client_secret_input;
    }
    if (state.oidc_trust_unverified_email !== initial.oidc_trust_unverified_email) {
      patch["auth.oidc.trust_unverified_email"] = state.oidc_trust_unverified_email;
    }

    await update.mutateAsync(patch);
    // Clear the secret input on success — `oidc_client_secret_set` will
    // flip true on the next query refresh.
    setState((s) => ({ ...s, oidc_client_secret_input: "" }));
  }

  async function onProbe() {
    setDiscoverResult({ kind: "idle" });
    try {
      const r = await probe.mutateAsync({ issuer: state.oidc_issuer });
      if (r) setDiscoverResult({ kind: "ok", data: r });
    } catch (e) {
      setDiscoverResult({
        kind: "err",
        message: e instanceof Error ? e.message : String(e),
      });
    }
  }

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        {/* Mode card */}
        <Card>
          <CardHeader>
            <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
              Mode
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <SegmentedControl
              value={state.mode}
              onChange={(mode) => setState((s) => ({ ...s, mode }))}
              ariaLabel="Auth mode"
              options={[
                { value: "local", label: "Local" },
                { value: "oidc", label: "OIDC" },
                { value: "both", label: "Both" },
              ]}
            />
            <p className="text-muted-foreground text-xs">
              <strong>Local</strong> uses email+password.{" "}
              <strong>OIDC</strong> uses an external IdP only.{" "}
              <strong>Both</strong> shows both sign-in CTAs.
            </p>
          </CardContent>
        </Card>

        {/* Local card */}
        <Card>
          <CardHeader>
            <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
              Local auth
              {!localEnabled && (
                <Badge variant="outline" className="ml-2 text-xs">
                  disabled by mode
                </Badge>
              )}
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="flex items-center justify-between">
              <div>
                <Label className="text-sm">Self-serve registration</Label>
                <p className="text-muted-foreground text-xs">
                  When off, the sign-up form is hidden. New users must be
                  invited or provisioned by an admin.
                </p>
              </div>
              <Switch
                checked={state.registration_open}
                onCheckedChange={(v) =>
                  setState((s) => ({ ...s, registration_open: v }))
                }
                disabled={!localEnabled}
              />
            </div>
          </CardContent>
        </Card>

        {/* OIDC card */}
        <Card className="lg:col-span-2">
          <CardHeader>
            <CardTitle className="text-muted-foreground flex items-center gap-2 text-sm font-semibold tracking-wide uppercase">
              <ShieldCheck className="h-4 w-4" />
              OIDC
              {!oidcEnabled && (
                <Badge variant="outline" className="ml-2 text-xs">
                  disabled by mode
                </Badge>
              )}
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="oidc-issuer">Issuer</Label>
              <Input
                id="oidc-issuer"
                placeholder="https://idp.example.com"
                value={state.oidc_issuer}
                onChange={(e) =>
                  setState((s) => ({ ...s, oidc_issuer: e.target.value }))
                }
                disabled={!oidcEnabled}
              />
              {issuerMissing && (
                <FieldHint tone="error">
                  Issuer is required when OIDC is enabled.
                </FieldHint>
              )}
              <div className="flex items-center gap-2 pt-1">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={onProbe}
                  disabled={
                    !state.oidc_issuer.trim() || probe.isPending || !oidcEnabled
                  }
                >
                  <Search className="mr-1.5 h-3.5 w-3.5" />
                  {probe.isPending ? "Probing…" : "Test discovery"}
                </Button>
                <DiscoverResult result={discoverResult} />
              </div>
            </div>

            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="oidc-client-id">Client ID</Label>
                <Input
                  id="oidc-client-id"
                  value={state.oidc_client_id}
                  onChange={(e) =>
                    setState((s) => ({ ...s, oidc_client_id: e.target.value }))
                  }
                  disabled={!oidcEnabled}
                />
                {clientIdMissing && (
                  <FieldHint tone="error">
                    Client ID is required.
                  </FieldHint>
                )}
              </div>
              <div className="space-y-2">
                <Label htmlFor="oidc-client-secret">Client secret</Label>
                <Input
                  id="oidc-client-secret"
                  type="password"
                  autoComplete="new-password"
                  placeholder={
                    state.oidc_client_secret_set ? "•••••••• (unchanged)" : ""
                  }
                  value={state.oidc_client_secret_input}
                  onChange={(e) =>
                    setState((s) => ({
                      ...s,
                      oidc_client_secret_input: e.target.value,
                    }))
                  }
                  disabled={!oidcEnabled}
                />
                {secretMissing && (
                  <FieldHint tone="error">
                    Client secret is required.
                  </FieldHint>
                )}
                <p className="text-muted-foreground text-xs">
                  Stored encrypted at rest. Never echoed back over the API.
                </p>
              </div>
            </div>

            <div className="flex items-center justify-between rounded-md border border-amber-500/30 bg-amber-500/5 p-3">
              <div>
                <Label className="text-sm">Trust unverified email</Label>
                <p className="text-amber-200/80 text-xs">
                  When ON, accept the `email` claim even if{" "}
                  <code>email_verified</code> is false. Materially weakens
                  email-claim trust; only enable if your IdP doesn&rsquo;t
                  emit the flag.
                </p>
              </div>
              <Switch
                checked={state.oidc_trust_unverified_email}
                onCheckedChange={(v) =>
                  setState((s) => ({
                    ...s,
                    oidc_trust_unverified_email: v,
                  }))
                }
                disabled={!oidcEnabled}
              />
            </div>
            {state.oidc_trust_unverified_email && (
              <div className="flex items-start gap-2 rounded-md border border-red-500/30 bg-red-500/5 p-3 text-xs text-red-300">
                <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                <span>
                  OIDC users with unverified email will be accepted. Any
                  takeover of the IdP&rsquo;s mailer affects this Folio
                  instance.
                </span>
              </div>
            )}
          </CardContent>
        </Card>
      </div>

      <div className="flex items-center justify-end gap-3">
        {oidcEnabled && oidcIncomplete && (
          <span className="text-xs text-amber-300">
            OIDC fields incomplete — save will be rejected by the server.
          </span>
        )}
        <Button onClick={onSave} disabled={!dirty || update.isPending}>
          {update.isPending ? "Saving…" : "Save changes"}
        </Button>
      </div>
    </div>
  );
}

function FieldHint({
  tone,
  children,
}: {
  tone: "error";
  children: React.ReactNode;
}) {
  return (
    <p
      className={
        tone === "error"
          ? "text-xs text-red-400"
          : "text-muted-foreground text-xs"
      }
    >
      {children}
    </p>
  );
}

function DiscoverResult({
  result,
}: {
  result:
    | { kind: "idle" }
    | { kind: "ok"; data: OidcDiscoverResp }
    | { kind: "err"; message: string };
}) {
  if (result.kind === "idle") return null;
  if (result.kind === "err") {
    return (
      <span className="text-xs text-red-400">
        Discovery failed: {result.message}
      </span>
    );
  }
  const d = result.data;
  return (
    <details className="text-xs">
      <summary className="cursor-pointer text-emerald-300">
        Discovered — {countEndpoints(d)} endpoint(s)
      </summary>
      <pre className="bg-muted text-foreground mt-2 max-w-full overflow-auto rounded p-2 text-[11px]">
        {JSON.stringify(d, null, 2)}
      </pre>
    </details>
  );
}

function countEndpoints(d: OidcDiscoverResp): number {
  return [
    d.authorization_endpoint,
    d.token_endpoint,
    d.jwks_uri,
    d.end_session_endpoint,
    d.userinfo_endpoint,
  ].filter(Boolean).length;
}
