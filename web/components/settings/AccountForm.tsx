"use client";

import { useEffect, useSyncExternalStore } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";

import { Button } from "@/components/ui/button";
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { getCsrfToken } from "@/lib/api/auth-refresh";
import { useMe } from "@/lib/api/queries";
import { useUpdateAccount, useUpdatePreferences } from "@/lib/api/mutations";

import { SessionsCard } from "./SessionsCard";
import { SettingsSection } from "./SettingsSection";

/** Read the `__Host-comic_csrf` cookie value so the form ships a hidden
 *  `csrf_token` field for the progressive-enhancement no-JS path. The
 *  cookie is set Secure but not HttpOnly precisely so JS can mirror it
 *  into the form. `useSyncExternalStore` keeps the value reactive across
 *  cookie rotations (e.g. after a refresh-token round-trip mints a new
 *  CSRF cookie) without falling foul of the `set-state-in-effect` lint. */
function subscribeCsrfNoop(): () => void {
  // `document.cookie` doesn't emit change events, so there's nothing to
  // subscribe to. Returning a no-op is sufficient — re-renders triggered
  // by other state will re-read the snapshot.
  return () => {};
}
function getCsrfSnapshot(): string {
  return getCsrfToken() ?? "";
}
function getCsrfServerSnapshot(): string {
  return "";
}
function useCsrfToken(): string {
  return useSyncExternalStore(
    subscribeCsrfNoop,
    getCsrfSnapshot,
    getCsrfServerSnapshot,
  );
}

const profileSchema = z.object({
  display_name: z.string().min(1, "Display name is required").max(120),
  email: z
    .string()
    .email("Enter a valid email")
    .or(z.literal("")) // tolerate empty for OIDC users showing the read-only field
    .default(""),
});
type ProfileValues = z.infer<typeof profileSchema>;

const passwordSchema = z
  .object({
    current_password: z.string().min(1, "Required"),
    new_password: z.string().min(12, "Must be at least 12 characters"),
    confirm_password: z.string(),
  })
  .refine((d) => d.new_password === d.confirm_password, {
    path: ["confirm_password"],
    message: "Passwords don't match",
  });
type PasswordValues = z.infer<typeof passwordSchema>;

export function AccountForm() {
  const me = useMe();
  if (me.isLoading) return <Skeleton className="h-72 w-full" />;
  if (me.error || !me.data) {
    return <p className="text-destructive text-sm">Failed to load account.</p>;
  }
  // The server distinguishes auth modes via `users.external_id` ("local:" vs
  // "oidc:"). The client only sees a redacted MeView, so we infer from the
  // server response when we hit /me/account: a 403 with code
  // `auth.email_managed_by_issuer` means OIDC. We optimistically allow the
  // edits and let the server decide; the toast on failure is clear.
  return (
    <div className="space-y-6">
      <ProfileCard me={me.data} />
      <SidebarPrefsCard me={me.data} />
      <PasswordCard />
      <SessionsCard />
    </div>
  );
}

/** Small bag of binary sidebar/navigation prefs. Today it's just the
 *  Bookmarks count badge; add additional toggles here as they come up
 *  rather than scattering one-off cards across /settings. */
function SidebarPrefsCard({
  me,
}: {
  me: NonNullable<ReturnType<typeof useMe>["data"]>;
}) {
  const update = useUpdatePreferences({ silent: false });
  const showMarkerCount = me.show_marker_count === true;
  return (
    <SettingsSection
      title="Sidebar"
      description="Tweak what shows up on the persistent left-rail."
    >
      <div className="flex items-start justify-between gap-6">
        <div className="space-y-0.5">
          <p className="text-foreground text-sm font-medium">
            Show bookmark count badge
          </p>
          <p className="text-muted-foreground max-w-prose text-sm">
            When on, the Bookmarks row in the sidebar shows a count badge for
            every marker you&rsquo;ve saved (bookmarks, notes, favorites,
            highlights). Default off for a quieter nav.
          </p>
        </div>
        <div className="shrink-0 pt-1">
          <Switch
            checked={showMarkerCount}
            onCheckedChange={(v) => update.mutate({ show_marker_count: v })}
            disabled={update.isPending}
            aria-label="Show bookmark count badge"
          />
        </div>
      </div>
    </SettingsSection>
  );
}

function ProfileCard({
  me,
}: {
  me: NonNullable<ReturnType<typeof useMe>["data"]>;
}) {
  const update = useUpdateAccount();
  const csrf = useCsrfToken();
  const form = useForm<ProfileValues>({
    resolver: zodResolver(profileSchema),
    defaultValues: { display_name: me.display_name, email: me.email ?? "" },
  });

  useEffect(() => {
    form.reset({ display_name: me.display_name, email: me.email ?? "" });
  }, [me.display_name, me.email, form]);

  const onSubmit = form.handleSubmit((values) => {
    const body: { display_name?: string; email?: string } = {};
    if (values.display_name !== me.display_name)
      body.display_name = values.display_name;
    if (values.email && values.email !== (me.email ?? ""))
      body.email = values.email;
    if (Object.keys(body).length === 0) return;
    update.mutate(body);
  });

  return (
    <SettingsSection
      title="Profile"
      description="Your display name shows up next to your activity in the admin views."
    >
      <Form {...form}>
        {/*
          Progressive enhancement. PATCH is the XHR contract used by the
          mutation hook; the no-JS fallback uses POST against the same
          handler (server routes both verbs to the same function). A
          hidden `csrf_token` field carries the double-submit token for
          the form path; the JSON path uses the `X-CSRF-Token` header.
        */}
        <form
          onSubmit={onSubmit}
          method="POST"
          action="/api/me/account"
          className="space-y-5"
        >
          {csrf ? (
            <input type="hidden" name="csrf_token" value={csrf} />
          ) : null}
          <FormField
            control={form.control}
            name="display_name"
            render={({ field }) => (
              <FormItem>
                <FormLabel>Display name</FormLabel>
                <FormControl>
                  <Input {...field} autoComplete="nickname" maxLength={120} />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={form.control}
            name="email"
            render={({ field }) => (
              <FormItem>
                <FormLabel>Email</FormLabel>
                <FormControl>
                  <Input {...field} type="email" autoComplete="email" />
                </FormControl>
                <FormDescription>
                  OIDC accounts can&apos;t change their email here — it&apos;s
                  managed by your identity provider.
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
          <div className="flex justify-end">
            <Button
              type="submit"
              disabled={update.isPending || !form.formState.isDirty}
            >
              {update.isPending ? "Saving…" : "Save profile"}
            </Button>
          </div>
        </form>
      </Form>
    </SettingsSection>
  );
}

function PasswordCard() {
  const update = useUpdateAccount();
  const csrf = useCsrfToken();
  const form = useForm<PasswordValues>({
    resolver: zodResolver(passwordSchema),
    defaultValues: {
      current_password: "",
      new_password: "",
      confirm_password: "",
    },
  });

  const onSubmit = form.handleSubmit(async (values) => {
    update.mutate(
      {
        current_password: values.current_password,
        new_password: values.new_password,
      },
      {
        onSuccess: () => form.reset(),
      },
    );
  });

  return (
    <SettingsSection
      title="Change password"
      description="Local accounts only. After a password change, all your other sessions are signed out."
    >
      <Form {...form}>
        {/* Progressive enhancement — see ProfileCard for rationale. */}
        <form
          onSubmit={onSubmit}
          method="POST"
          action="/api/me/account"
          className="space-y-5"
        >
          {csrf ? (
            <input type="hidden" name="csrf_token" value={csrf} />
          ) : null}
          <FormField
            control={form.control}
            name="current_password"
            render={({ field }) => (
              <FormItem>
                <FormLabel>Current password</FormLabel>
                <FormControl>
                  <Input
                    {...field}
                    type="password"
                    autoComplete="current-password"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={form.control}
            name="new_password"
            render={({ field }) => (
              <FormItem>
                <FormLabel>New password</FormLabel>
                <FormControl>
                  <Input
                    {...field}
                    type="password"
                    autoComplete="new-password"
                  />
                </FormControl>
                <FormDescription>At least 12 characters.</FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={form.control}
            name="confirm_password"
            render={({ field }) => (
              <FormItem>
                <FormLabel>Confirm new password</FormLabel>
                <FormControl>
                  <Input
                    {...field}
                    type="password"
                    autoComplete="new-password"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <div className="flex justify-end">
            <Button type="submit" disabled={update.isPending}>
              {update.isPending ? "Saving…" : "Change password"}
            </Button>
          </div>
        </form>
      </Form>
    </SettingsSection>
  );
}
