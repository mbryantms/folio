"use client";

import { useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { AlertTriangle, KeyRound } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";

const schema = z
  .object({
    new_password: z.string().min(12, "Must be at least 12 characters"),
    confirm_password: z.string(),
  })
  .refine((d) => d.new_password === d.confirm_password, {
    path: ["confirm_password"],
    message: "Passwords don't match",
  });
type Values = z.infer<typeof schema>;

export function ResetPasswordForm({ token }: { token: string | null }) {
  const form = useForm<Values>({
    resolver: zodResolver(schema),
    defaultValues: { new_password: "", confirm_password: "" },
  });
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  if (!token) {
    return (
      <Card>
        <CardHeader className="space-y-2 text-center">
          <AlertTriangle className="text-destructive mx-auto size-8" />
          <CardTitle className="text-xl">Missing reset token</CardTitle>
          <CardDescription>
            The reset link is incomplete. Open the link from your email again,
            or request a new one.
          </CardDescription>
        </CardHeader>
        <CardFooter className="justify-center pt-0">
          <Link
            href="/forgot-password"
            className="text-muted-foreground hover:text-foreground text-xs underline-offset-4 hover:underline"
          >
            Request a new reset link
          </Link>
        </CardFooter>
      </Card>
    );
  }

  // Auth forms use inline error banners + <FormMessage> instead of
  // toasts; success is signalled by route navigation. See
  // docs/dev/notifications-audit.md §F-6 for the standard.
  const onSubmit = form.handleSubmit(async (values) => {
    setError(null);
    setSubmitting(true);
    try {
      const res = await fetch("/auth/local/reset-password", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "include",
        body: JSON.stringify({
          token,
          new_password: values.new_password,
        }),
      });
      if (!res.ok) {
        try {
          const body = await res.json();
          setError(body?.error?.message ?? "Reset failed.");
        } catch {
          setError(`HTTP ${res.status}`);
        }
        return;
      }
      router.push("/sign-in?reset=1");
    } finally {
      setSubmitting(false);
    }
  });

  return (
    <Card>
      <CardHeader className="space-y-2 text-center">
        <KeyRound className="text-muted-foreground mx-auto size-8" />
        <CardTitle className="text-xl">Set a new password</CardTitle>
        <CardDescription>
          Choose a new password. After updating, all your existing sessions will
          be signed out.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <Form {...form}>
          {/*
            Progressive enhancement — see SignInClient.LoginForm. The
            handler at `/auth/local/reset-password` accepts both JSON
            (XHR happy path) and form-encoded (no-JS fallback) bodies; on
            the form path it 303s to `/sign-in?reset=1` on success or back
            to this page with `?token=…&error=…` on failure.
          */}
          <form
            onSubmit={onSubmit}
            method="POST"
            action="/auth/local/reset-password"
            className="space-y-4"
          >
            <input type="hidden" name="token" value={token} />
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
                  <p className="text-muted-foreground text-xs">
                    Must be at least 12 characters.
                  </p>
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
            {error ? (
              <p
                role="alert"
                className="border-destructive/40 bg-destructive/10 text-destructive rounded-md border px-3 py-2 text-sm"
              >
                {error}
              </p>
            ) : null}
            <Button type="submit" disabled={submitting} className="w-full">
              {submitting ? "Updating…" : "Update password"}
            </Button>
          </form>
        </Form>
      </CardContent>
    </Card>
  );
}
