"use client";

/**
 * Friendly per-provider credential form (metadata-providers-1.0 M6).
 *
 * Writes to the same `PATCH /admin/settings` endpoint the generic
 * settings page uses, but presents labeled inputs ("ComicVine API
 * key", "Metron password") instead of forcing the operator to find
 * `metadata.comicvine.api_key` in a flat list. Secret values come
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
      username: string;
      password: string;
      passwordAlreadySet: boolean;
      enabled: boolean;
    };

export function ProviderConfigForm({
  provider,
}: {
  provider: "comicvine" | "metron";
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
      : `metron-${initial.username}-${initial.passwordAlreadySet ? "1" : "0"}-${initial.enabled ? "1" : "0"}`;

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
  provider: "comicvine" | "metron",
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
  const passRaw = str("metadata.metron.password");
  return {
    kind: "metron",
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
  provider: "comicvine" | "metron";
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
  const [username, setUsername] = React.useState(
    initial.kind === "metron" ? initial.username : "",
  );
  const [password, setPassword] = React.useState(
    initial.kind === "metron" ? initial.password : "",
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
    } else {
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
    username,
    password,
    enabled,
  });

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
      ) : (
        <>
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
          <span>
            Enable {provider === "comicvine" ? "ComicVine" : "Metron"}
          </span>
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
  provider: "comicvine" | "metron",
  initial: CredentialFields,
  current: {
    apiKey: string;
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
    if (current.username !== initial.username) return true;
    if (current.password !== "" && current.password !== initial.password) {
      return true;
    }
  }
  return false;
}
