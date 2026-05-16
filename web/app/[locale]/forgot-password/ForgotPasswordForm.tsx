"use client";

import { useState } from "react";
import Link from "next/link";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { CheckCircle2, Mail } from "lucide-react";

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

const schema = z.object({
  email: z.string().email("Enter a valid email"),
});
type Values = z.infer<typeof schema>;

export function ForgotPasswordForm({
  preSubmitted = false,
}: {
  /** True when the page arrived here via the no-JS form-fallback 303
   *  (`/forgot-password?sent=1`). Renders the "check your email" view
   *  without the email-address echo, since the form-encoded body never
   *  reached client state. */
  preSubmitted?: boolean;
} = {}) {
  const form = useForm<Values>({
    resolver: zodResolver(schema),
    defaultValues: { email: "" },
  });
  const [submitted, setSubmitted] = useState(preSubmitted);
  const [submittedEmail, setSubmittedEmail] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // Auth forms use inline error banners + <FormMessage> instead of
  // toasts; success is signalled by route navigation or alternate view
  // state. See docs/dev/notifications-audit.md §F-6 for the standard.
  const onSubmit = form.handleSubmit(async (values) => {
    setSubmitting(true);
    try {
      // Always treat the request as accepted — the server returns 204
      // whether or not the email maps to a real account (no enumeration).
      await fetch("/auth/local/request-password-reset", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "include",
        body: JSON.stringify(values),
      });
      setSubmittedEmail(values.email);
      setSubmitted(true);
    } finally {
      setSubmitting(false);
    }
  });

  if (submitted) {
    return (
      <Card>
        <CardHeader className="space-y-2 text-center">
          <CheckCircle2 className="mx-auto size-8 text-emerald-400" />
          <CardTitle className="text-xl">Check your email</CardTitle>
          <CardDescription>
            {submittedEmail ? (
              <>
                If <strong>{submittedEmail}</strong> matches an account, a
                password-reset link is on its way. The link expires in 1 hour.
              </>
            ) : (
              <>
                If that address matches an account, a password-reset link is
                on its way. The link expires in 1 hour.
              </>
            )}
          </CardDescription>
        </CardHeader>
        <CardFooter className="justify-center pt-0">
          <Link
            href="/sign-in"
            className="text-muted-foreground hover:text-foreground text-xs underline-offset-4 hover:underline"
          >
            Back to sign-in
          </Link>
        </CardFooter>
      </Card>
    );
  }

  return (
    <Card>
      <CardHeader className="space-y-2 text-center">
        <Mail className="text-muted-foreground mx-auto size-8" />
        <CardTitle className="text-xl">Reset your password</CardTitle>
        <CardDescription>
          Enter the email address on your account and we&rsquo;ll send a reset
          link.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <Form {...form}>
          {/*
            Progressive enhancement — the form posts to a real endpoint
            so a pre-hydration submit doesn't fall through to a GET
            (which would leak `?email=` into the URL bar and Referer).
            Server handles both JSON (XHR happy path) and form-encoded
            (no-JS fallback) bodies.
          */}
          <form
            onSubmit={onSubmit}
            method="POST"
            action="/auth/local/request-password-reset"
            className="space-y-4"
          >
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
                      autoCapitalize="none"
                      autoCorrect="off"
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <Button type="submit" disabled={submitting} className="w-full">
              {submitting ? "Sending…" : "Send reset link"}
            </Button>
          </form>
        </Form>
      </CardContent>
      <CardFooter className="justify-center pt-0">
        <Link
          href="/sign-in"
          className="text-muted-foreground hover:text-foreground text-xs underline-offset-4 hover:underline"
        >
          Back to sign-in
        </Link>
      </CardFooter>
    </Card>
  );
}
