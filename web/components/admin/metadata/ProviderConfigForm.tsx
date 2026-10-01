"use client";

/**
 * Friendly per-provider credential form (metadata-providers-1.0 M6).
 *
 * Writes to the same `PATCH /admin/settings` endpoint the generic
 * settings page uses, but presents labeled inputs ("ComicVine API
 * key", "Metron API token") instead of forcing the operator to find
 * `metadata.comicvine.api_key` in a flat list. Metron prefers its API
 * token (WP-2.9); the username + password pair stays as the fallback
 * the server uses when no token is set. GCD (WP-6.1) takes a
 * comics.org username + password (HTTP Basic). Secret values come
 * back from `GET /admin/settings` as the sentinel string `"<set>"`
 * — the form shows a "(saved)" placeholder + leaves the input
 * empty so re-saving without typing is a no-op.
 *
 * Mounted inside `<ProvidersTab>` once per provider; on success the
 * cache invalidations refresh the dashboard counts + the per-provider
 * quota snapshot so the operator sees the new state immediately.
 */

import { CheckCircle2, Loader2 } from "lucide-react";
import * as React from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { useUpdateSettings } from "@/lib/api/mutations";
import { useAdminSettings } from "@/lib/api/queries";
import { useUnsavedChangesGuard } from "@/lib/ui/use-unsaved-changes-guard";
import { statusToneText } from "@/lib/ui/status-tone";

/** Sentinel the server returns for secret values that have been set
 *  (the plaintext is never sent to the client). */
const SECRET_SET = "<set>";

type CredentialFields =
  | {
      kind: "comicvine";
      apiKey: string;
      apiKeyAlreadySet: boolean;
      enabled: boolean;
    }
  | {
      kind: "metron";
      apiToken: string;
      apiTokenAlreadySet: boolean;
      username: string;
      password: string;
      passwordAlreadySet: boolean;
      enabled: boolean;
    }
  | {
      kind: "gcd";
      username: string;
      password: string;
      passwordAlreadySet: boolean;
      enabled: boolean;
    };

export type ConfigurableProvider = "comicvine" | "metron" | "gcd";

const PROVIDER_NAMES: Record<ConfigurableProvider, string> = {
  comicvine: "ComicVine",
  metron: "Metron",
  gcd: "GCD",
};

export function isConfigurableProvider(id: string): id is ConfigurableProvider {
  return id === "comicvine" || id === "metron" || id === "gcd";
}

export function ProviderConfigForm({
  provider,
}: {
  provider: ConfigurableProvider;
}) {
  const settings = useAdminSettings();
  const update = useUpdateSettings();
  const [savedFlash, setSavedFlash] = React.useState(false);

  if (settings.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-2 text-xs">
        <Loader2 className="h-3 w-3 animate-spin" /> Loading credentials…
      </div>
    );
  }
  if (!settings.data) {
    return null;
  }
  // `values` arrives as a parallel array of `{ key, value, is_secret }`
  // rows in registry order. Flatten to a keyed lookup so the per-key
  // reads below stay O(1).
  const byKey: Record<string, unknown> = {};
  for (const row of settings.data.values) {
    byKey[row.key] = row.value;
  }
  const initial = readInitial(provider, byKey);

  // `key` forces a remount of the inner form on any change to the
  // saved settings (post-mutation refetch flips the SECRET_SET
  // sentinel) so the form re-seeds from props without a derived-state
  // useEffect. Includes the boolean toggle + the "is the secret set"
  // bit so toggling enabled-from-elsewhere also re-syncs.
  const formKey =
    initial.kind === "comicvine"
      ? `cv-${initial.apiKeyAlreadySet ? "1" : "0"}-${initial.enabled ? "1" : "0"}`
      : initial.kind === "metron"
        ? `metron-${initial.username}-${initial.apiTokenAlreadySet ? "1" : "0"}-${initial.passwordAlreadySet ? "1" : "0"}-${initial.enabled ? "1" : "0"}`
        : `gcd-${initial.username}-${initial.passwordAlreadySet ? "1" : "0"}-${initial.enabled ? "1" : "0"}`;

  return (
    <ProviderForm
      key={formKey}
      provider={provider}
      initial={initial}
      isPending={update.isPending}
      savedFlash={savedFlash}
      onSubmit={async (patch) => {
        if (Object.keys(patch).length === 0) return;
        try {
          await update.mutateAsync(patch);
          setSavedFlash(true);
          window.setTimeout(() => setSavedFlash(false), 2000);
        } catch {
          // useApiMutation already toasts on error.
        }
      }}
    />
  );
}

function readInitial(
  provider: ConfigurableProvider,
  values: Record<string, unknown>,
): CredentialFields {
  const str = (k: string) =>
    typeof values[k] === "string" ? (values[k] as string) : "";
  const bool = (k: string) =>
    typeof values[k] === "boolean" ? (values[k] as boolean) : false;
  if (provider === "comicvine") {
    const raw = str("metadata.comicvine.api_key");
    return {
      kind: "comicvine",
      apiKey: raw === SECRET_SET ? "" : raw,
      apiKeyAlreadySet: raw === SECRET_SET,
      enabled: bool("metadata.comicvine.enabled"),
    };
  }
  if (provider === "gcd") {
    const gcdPass = str("metadata.gcd.password");
    return {
      kind: "gcd",
      username: str("metadata.gcd.username"),
      password: gcdPass === SECRET_SET ? "" : gcdPass,
      passwordAlreadySet: gcdPass === SECRET_SET,
      enabled: bool("metadata.gcd.enabled"),
    };
  }
  const passRaw = str("metadata.metron.password");
  const tokenRaw = str("metadata.metron.api_token");
  return {
    kind: "metron",
    apiToken: tokenRaw === SECRET_SET ? "" : tokenRaw,
    apiTokenAlreadySet: tokenRaw === SECRET_SET,
    username: str("metadata.metron.username"),
    password: passRaw === SECRET_SET ? "" : passRaw,
    passwordAlreadySet: passRaw === SECRET_SET,
    enabled: bool("metadata.metron.enabled"),
  };
}

function ProviderForm({
  provider,
  initial,
  isPending,
  savedFlash,
  onSubmit,
}: {
  provider: ConfigurableProvider;
  initial: CredentialFields;
  isPending: boolean;
  savedFlash: boolean;
  onSubmit: (patch: Record<string, unknown>) => Promise<void>;
}) {
  // Internal state, seeded from props. The outer `key=` on this
  // component remounts it whenever the saved settings change, so we
  // don't need a derived-state useEffect.
  const [apiKey, setApiKey] = React.useState(
    initial.kind === "comicvine" ? initial.apiKey : "",
  );
  const [apiToken, setApiToken] = React.useState(
    initial.kind === "metron" ? initial.apiToken : "",
  );
  const [username, setUsername] = React.useState(
    initial.kind === "metron" || initial.kind === "gcd" ? initial.username : "",
  );
  const [password, setPassword] = React.useState(
    initial.kind === "metron" || initial.kind === "gcd" ? initial.password : "",
  );
  const [enabled, setEnabled] = React.useState(initial.enabled);

  const handle = (e: React.FormEvent) => {
    e.preventDefault();
    const patch: Record<string, unknown> = {};
    if (initial.kind === "comicvine") {
      // Trim before send — pasting the CV API key from their site
      // commonly drags a trailing newline that CV rejects as Invalid.
      const trimmed = apiKey.trim();
      if (trimmed !== "" && trimmed !== initial.apiKey) {
        patch["metadata.comicvine.api_key"] = trimmed;
      }
      if (enabled !== initial.enabled) {
        patch["metadata.comicvine.enabled"] = enabled;
      }
    } else if (initial.kind === "gcd") {
      const trimmedUser = username.trim();
      const trimmedPass = password.trim();
      if (trimmedUser !== initial.username) {
        patch["metadata.gcd.username"] =
          trimmedUser === "" ? null : trimmedUser;
      }
      if (trimmedPass !== "" && trimmedPass !== initial.password) {
        patch["metadata.gcd.password"] = trimmedPass;
      }
      if (enabled !== initial.enabled) {
        patch["metadata.gcd.enabled"] = enabled;
      }
    } else {
      const trimmedToken = apiToken.trim();
      if (trimmedToken !== "" && trimmedToken !== initial.apiToken) {
        patch["metadata.metron.api_token"] = trimmedToken;
      }
      const trimmedUser = username.trim();
      const trimmedPass = password.trim();
      if (trimmedUser !== initial.username) {
        patch["metadata.metron.username"] =
          trimmedUser === "" ? null : trimmedUser;
      }
      if (trimmedPass !== "" && trimmedPass !== initial.password) {
        patch["metadata.metron.password"] = trimmedPass;
      }
      if (enabled !== initial.enabled) {
        patch["metadata.metron.enabled"] = enabled;
      }
    }
    void onSubmit(patch);
  };

  const dirty = isDirty(provider, initial, {
    apiKey,
    apiToken,
    username,
    password,
    enabled,
  });
  useUnsavedChangesGuard(dirty);

  return (
    <form
      onSubmit={handle}
      className="border-border space-y-3 rounded border-t pt-3"
      aria-label={`${provider} credentials`}
    >
      {initial.kind === "comicvine" ? (
        <div className="grid gap-1.5">
          <Label htmlFor="cv-api-key">API key</Label>
          <Input
            id="cv-api-key"
            type="password"
            autoComplete="off"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder={
              initial.apiKeyAlreadySet
                ? "(saved — type to replace)"
                : "Paste your ComicVine API key"
            }
          />
        </div>
      ) : initial.kind === "gcd" ? (
        <>
          <div className="grid gap-1.5">
            <Label htmlFor="gcd-username">Username</Label>
            <Input
              id="gcd-username"
              autoComplete="off"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              placeholder="comics.org account"
            />
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="gcd-password">Password</Label>
            <Input
              id="gcd-password"
              type="password"
              autoComplete="off"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder={
                initial.passwordAlreadySet
                  ? "(saved — type to replace)"
                  : "comics.org password"
              }
            />
            <p className="text-muted-foreground text-xs">
              A free comics.org account lifts the API limit from 30 requests an
              hour (anonymous) to 2,000 a day.
            </p>
          </div>
        </>
      ) : (
        <>
          <div className="grid gap-1.5">
            <Label htmlFor="metron-api-token">API token</Label>
            <Input
              id="metron-api-token"
              type="password"
              autoComplete="off"
              value={apiToken}
              onChange={(e) => setApiToken(e.target.value)}
              placeholder={
                initial.apiTokenAlreadySet
                  ? "(saved — type to replace)"
                  : "Paste your Metron API token"
              }
            />
            <p className="text-muted-foreground text-xs">
              Generate one under <span className="font-medium">API Tokens</span>{" "}
              on your metron.cloud account page. Preferred; the username +
              password below are only used when no token is set.
            </p>
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="metron-username">Username</Label>
            <Input
              id="metron-username"
              autoComplete="off"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              placeholder="metron.cloud account"
            />
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="metron-password">Password</Label>
            <Input
              id="metron-password"
              type="password"
              autoComplete="off"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder={
                initial.passwordAlreadySet
                  ? "(saved — type to replace)"
                  : "metron.cloud password"
              }
            />
          </div>
        </>
      )}

      <div className="flex items-center justify-between gap-2">
        <Label
          htmlFor={`${provider}-enabled`}
          className="flex cursor-pointer items-center gap-2 text-sm"
        >
          <Switch
            id={`${provider}-enabled`}
            checked={enabled}
            onCheckedChange={setEnabled}
          />
          <span>Enable {PROVIDER_NAMES[provider]}</span>
        </Label>
        <div className="flex items-center gap-2">
          {savedFlash && (
            <span className={`text-xs ${statusToneText("success")}`}>
              <CheckCircle2 className="mr-1 inline h-3 w-3" /> Saved
            </span>
          )}
          <Button type="submit" size="sm" disabled={!dirty || isPending}>
            {isPending ? (
              <>
                <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Saving
              </>
            ) : (
              "Save"
            )}
          </Button>
        </div>
      </div>
    </form>
  );
}

function isDirty(
  provider: ConfigurableProvider,
  initial: CredentialFields,
  current: {
    apiKey: string;
    apiToken: string;
    username: string;
    password: string;
    enabled: boolean;
  },
): boolean {
  if (current.enabled !== initial.enabled) return true;
  if (provider === "comicvine" && initial.kind === "comicvine") {
    return current.apiKey !== "" && current.apiKey !== initial.apiKey;
  }
  if (provider === "metron" && initial.kind === "metron") {
    if (current.apiToken !== "" && current.apiToken !== initial.apiToken) {
      return true;
    }
    if (current.username !== initial.username) return true;
    if (current.password !== "" && current.password !== initial.password) {
      return true;
    }
  }
  if (provider === "gcd" && initial.kind === "gcd") {
    if (current.username !== initial.username) return true;
    if (current.password !== "" && current.password !== initial.password) {
      return true;
    }
  }
  return false;
}
