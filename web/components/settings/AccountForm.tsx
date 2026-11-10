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
} from "@/components/folio/form";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { getCsrfToken } from "@/lib/api/auth-refresh";
import { applyServerErrors } from "@/lib/api/form-errors";
import { useMe } from "@/lib/api/queries";
import {
  useCancelEmailChange,
  useUpdateAccount,
  useUpdatePreferences,
} from "@/lib/api/mutations";

import { DataExportCard } from "./DataExportCard";
import { DeleteAccountCard } from "./DeleteAccountCard";
import { HoldingsExportCard } from "./HoldingsExportCard";
import { SessionsCard } from "./SessionsCard";
import { SettingsSection } from "./SettingsSection";
import { LoadingRegion } from "@/components/skeletons/LoadingShapes";
import { AccountBodySkeleton } from "@/components/skeletons/SettingsShapes";

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
  // Asked for only when the email changes (and the account has a password).
  current_password: z.string().default(""),
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

/** Outcome of opening an email-change link, from the
 *  `?email_change=` query the server redirects to (or the no-JS form
 *  fallback's `sent`). `confirmed_signed_out`: the link was opened in a
 *  browser not signed in to that account, so no session of it was kept. */
export type EmailChangeStatus =
  "confirmed" | "confirmed_signed_out" | "invalid" | "taken" | "sent";

export function AccountForm({
  emailChange = null,
}: {
  emailChange?: EmailChangeStatus | null;
} = {}) {
  const me = useMe();
  if (me.isLoading) {
    return (
      <LoadingRegion label="Loading account…">
        <AccountBodySkeleton />
      </LoadingRegion>
    );
  }
  if (me.error || !me.data) {
    return <p className="text-destructive text-sm">Failed to load account.</p>;
  }
  return (
    <div className="space-y-6">
      <ProfileCard me={me.data} emailChange={emailChange} />
      <SidebarPrefsCard me={me.data} />
      <PasswordCard me={me.data} />
      <SessionsCard />
      <DataExportCard />
      <HoldingsExportCard />
      <DeleteAccountCard me={me.data} />
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

const PROFILE_FIELDS = ["display_name", "email", "current_password"] as const;
const PASSWORD_FIELDS = [
  "current_password",
  "new_password",
  "confirm_password",
] as const;

/** What the page shows after an email-change link was opened. */
function EmailChangeOutcome({ status }: { status: EmailChangeStatus | null }) {
  if (status === "confirmed") {
    return (
      <Alert>
        <AlertTitle>Email address changed</AlertTitle>
        <AlertDescription>
          Your new address is confirmed. Other devices were signed out.
        </AlertDescription>
      </Alert>
    );
  }
  if (status === "confirmed_signed_out") {
    return (
      <Alert>
        <AlertTitle>Email address changed</AlertTitle>
        <AlertDescription>
          The new address is confirmed. Every device signed in to that account
          was signed out; sign in again with the new address.
        </AlertDescription>
      </Alert>
    );
  }
  if (status === "invalid") {
    return (
      <Alert variant="destructive">
        <AlertTitle>That confirmation link didn&rsquo;t work</AlertTitle>
        <AlertDescription>
          It has expired, was already used, was cancelled or replaced by a newer
          request, or the account&rsquo;s password or sessions changed since it
          was sent. Your email address was not changed.
        </AlertDescription>
      </Alert>
    );
  }
  if (status === "taken") {
    return (
      <Alert variant="destructive">
        <AlertTitle>That address is already in use</AlertTitle>
        <AlertDescription>
          Another account now uses it, so your email address was not changed.
        </AlertDescription>
      </Alert>
    );
  }
  return null;
}

function ProfileCard({
  me,
  emailChange,
}: {
  me: NonNullable<ReturnType<typeof useMe>["data"]>;
  emailChange: EmailChangeStatus | null;
}) {
  const update = useUpdateAccount();
  const cancelChange = useCancelEmailChange();
  const csrf = useCsrfToken();
  const emailEditable = me.email_editable !== false;
  const passwordEditable = me.password_editable !== false;
  const pendingEmail = me.pending_email ?? null;
  const form = useForm<z.input<typeof profileSchema>, unknown, ProfileValues>({
    resolver: zodResolver(profileSchema),
    defaultValues: {
      display_name: me.display_name,
      email: me.email ?? "",
      current_password: "",
    },
  });

  useEffect(() => {
    form.reset({
      display_name: me.display_name,
      email: me.email ?? "",
      current_password: "",
    });
  }, [me.display_name, me.email, form]);

  // The password field is always in the markup (so the no-JS form POST
  // can carry it); it is only required once the email changes.
  const askPassword = emailEditable && passwordEditable;

  const onSubmit = form.handleSubmit((values) => {
    const body: {
      display_name?: string;
      email?: string;
      current_password?: string;
    } = {};
    if (values.display_name !== me.display_name)
      body.display_name = values.display_name;
    if (emailEditable && values.email && values.email !== (me.email ?? "")) {
      body.email = values.email;
      if (passwordEditable) {
        if (!values.current_password) {
          form.setError("current_password", {
            message: "Enter your current password to change your email",
          });
          return;
        }
        body.current_password = values.current_password;
      }
    }
    if (Object.keys(body).length === 0) return;
    update.mutate(body, {
      // A pending change leaves `email` as it was; reset so the field shows
      // the current address and the password is cleared.
      onSuccess: (data) => {
        if (data)
          form.reset({
            display_name: data.display_name,
            email: data.email ?? "",
            current_password: "",
          });
      },
      onError: (err) => {
        applyServerErrors(form.setError, err, [...PROFILE_FIELDS]);
      },
    });
  });

  return (
    <SettingsSection
      title="Profile"
      description="Your display name shows up next to your activity in the admin views."
    >
      <div className="space-y-5">
        <EmailChangeOutcome status={emailChange} />
        {pendingEmail ? (
          <Alert>
            <AlertTitle>Confirmation sent to {pendingEmail}</AlertTitle>
            <AlertDescription>
              <p>
                Open the link in that email to finish changing your address. It
                expires after an hour. Until then you keep using{" "}
                {me.email ?? "your current address"}.
              </p>
              {me.email ? (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  aria-label="Cancel email change"
                  disabled={cancelChange.isPending}
                  onClick={() => cancelChange.mutate({ email: me.email! })}
                >
                  {cancelChange.isPending ? "Cancelling…" : "Cancel"}
                </Button>
              ) : null}
            </AlertDescription>
          </Alert>
        ) : emailChange === "sent" ? (
          <Alert>
            <AlertTitle>Check your new address</AlertTitle>
            <AlertDescription>
              We sent a confirmation link to it. The change happens when you
              open the link.
            </AlertDescription>
          </Alert>
        ) : null}
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
                    <Input
                      {...field}
                      type="email"
                      autoComplete="email"
                      disabled={!emailEditable}
                    />
                  </FormControl>
                  <FormDescription>
                    {emailEditable
                      ? "Used for account notifications and local sign-in."
                      : "Email is managed by your identity provider."}
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
            {askPassword ? (
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
                    <FormDescription>
                      Only needed when you change your email. If this server
                      sends email, the change waits until you open the link sent
                      to the new address.
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
            ) : null}
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
      </div>
    </SettingsSection>
  );
}

function PasswordCard({
  me,
}: {
  me: NonNullable<ReturnType<typeof useMe>["data"]>;
}) {
  const passwordEditable = me.password_editable !== false;
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

  if (!passwordEditable) {
    return (
      <SettingsSection
        title="Change password"
        description="Password changes are managed by your identity provider."
      >
        <p className="text-muted-foreground text-sm">
          This account signs in through an external identity provider, so there
          is no local password to change here.
        </p>
      </SettingsSection>
    );
  }

  const onSubmit = form.handleSubmit(async (values) => {
    update.mutate(
      {
        current_password: values.current_password,
        new_password: values.new_password,
      },
      {
        onSuccess: () => form.reset(),
        // A wrong current password comes back as a 403 field error.
        onError: (err) => {
          applyServerErrors(form.setError, err, [...PASSWORD_FIELDS]);
        },
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
          {csrf ? <input type="hidden" name="csrf_token" value={csrf} /> : null}
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
