"use client";

import { useEffect, useState, useSyncExternalStore } from "react";
import { useRouter } from "next/navigation";
import Link from "next/link";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { CheckCircle2, Inbox, LogIn, Mail } from "lucide-react";

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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { PublicAuthConfigView } from "@/lib/api/types";

const loginSchema = z.object({
  email: z.string().email("Enter a valid email"),
  password: z.string().min(1, "Required"),
});
type LoginValues = z.infer<typeof loginSchema>;

const registerSchema = z.object({
  email: z.string().email("Enter a valid email"),
  password: z.string().min(12, "Must be at least 12 characters"),
});
type RegisterValues = z.infer<typeof registerSchema>;

type Banner = "verified" | "reset" | "pending" | "error" | null;

export function SignInClient({
  config,
  next,
  banner,
  errorMessage,
}: {
  config: PublicAuthConfigView;
  next: string | null;
  banner: Banner;
  errorMessage: string | null;
}) {
  const [mode, setMode] = useState<"login" | "register">("login");
  const [pendingEmail, setPendingEmail] = useState<string | null>(null);

  // Credential-in-URL scrubber. If this page was reached via an accidental
  // GET form submission (the regression that motivated M9), the address bar
  // — and the back-button history entry — currently contain
  // `?email=&password=`. As soon as we hydrate, replace the URL with a
  // clean one so the back-button entry is rewritten and any subsequent
  // copy-paste of the URL doesn't carry the credentials. This runs before
  // any other useEffect so the leak window is as small as we can make it
  // on the client side.
  useEffect(() => {
    if (typeof window === "undefined") return;
    const search = window.location.search;
    if (search.includes("password=") || search.includes("email=")) {
      const path = window.location.pathname + window.location.hash;
      window.history.replaceState(null, "", path);
    }
  }, []);

  // Insecure-context warning. `__Host-` cookies require Secure (HTTPS), so
  // submitting credentials over plain HTTP from a LAN IP succeeds at the
  // API layer but the browser silently drops the session cookies — the
  // user appears to remain on /sign-in with no error. Warn explicitly so
  // they don't lose ten minutes diagnosing what should be obvious.
  // Read via `useSyncExternalStore` so the snapshot is computed on the
  // client only (matches the SSR `false` snapshot until hydration).
  const insecureWarning = useSyncExternalStore(
    subscribeNoop,
    getInsecureContextSnapshot,
    getServerInsecureSnapshot,
  );

  const localEnabled = config.auth_mode !== "oidc";
  const showRegister = localEnabled && config.registration_open;
  const showOidc = config.oidc_enabled;

  if (pendingEmail) {
    return (
      <CenteredCard>
        <PendingVerificationView
          email={pendingEmail}
          onBack={() => setPendingEmail(null)}
        />
      </CenteredCard>
    );
  }

  return (
    <CenteredCard>
      <CardHeader className="space-y-2 text-center">
        <CardTitle className="text-2xl">Sign in to Folio</CardTitle>
        <CardDescription>Your self-hosted comic library.</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        {insecureWarning ? (
          <Banner tone="error">
            This page is served over plain HTTP. Sessions require HTTPS, so
            signing in here will not stick — your browser drops the
            session cookies. Use the HTTPS URL or visit via{" "}
            <code>localhost</code>.
          </Banner>
        ) : null}
        <BannerStrip banner={banner} errorMessage={errorMessage} />

        {showOidc ? <SsoButton next={next} /> : null}

        {showOidc && localEnabled ? (
          <div className="text-muted-foreground relative flex items-center text-xs">
            <span className="bg-border h-px flex-1" />
            <span className="px-3 tracking-wider uppercase">or</span>
            <span className="bg-border h-px flex-1" />
          </div>
        ) : null}

        {localEnabled ? (
          <Tabs
            value={mode}
            onValueChange={(v) => setMode(v as "login" | "register")}
          >
            <TabsList className="grid w-full grid-cols-2">
              <TabsTrigger value="login">Sign in</TabsTrigger>
              <TabsTrigger value="register" disabled={!showRegister}>
                Register
              </TabsTrigger>
            </TabsList>
            <TabsContent value="login" className="pt-4">
              <LoginForm next={next} />
            </TabsContent>
            <TabsContent value="register" className="pt-4">
              {showRegister ? (
                <RegisterForm next={next} onPending={setPendingEmail} />
              ) : (
                <p className="text-muted-foreground py-6 text-center text-sm">
                  Self-serve registration is disabled. Ask an administrator to
                  invite you.
                </p>
              )}
            </TabsContent>
          </Tabs>
        ) : (
          <p className="text-muted-foreground text-center text-sm">
            This deployment uses SSO only. Sign in with your identity provider
            above.
          </p>
        )}
      </CardContent>
      {localEnabled ? (
        <CardFooter className="justify-center pt-0">
          <Link
            href="/forgot-password"
            className="text-muted-foreground hover:text-foreground text-xs underline-offset-4 hover:underline"
          >
            Forgot your password?
          </Link>
        </CardFooter>
      ) : null}
    </CenteredCard>
  );
}

function subscribeNoop(): () => void {
  // Insecure-context never changes within a page load, so no subscription
  // is needed — `useSyncExternalStore` still requires a function.
  return () => {};
}

function getInsecureContextSnapshot(): boolean {
  if (typeof window === "undefined") return false;
  const { protocol, hostname } = window.location;
  const isLocal =
    hostname === "localhost" ||
    hostname === "127.0.0.1" ||
    hostname === "[::1]";
  return protocol !== "https:" && !isLocal;
}

function getServerInsecureSnapshot(): boolean {
  return false;
}

function CenteredCard({ children }: { children: React.ReactNode }) {
  return (
    <div className="bg-background flex min-h-screen items-center justify-center px-4 py-12">
      <Card className="w-full max-w-sm">{children}</Card>
    </div>
  );
}

function BannerStrip({
  banner,
  errorMessage,
}: {
  banner: Banner;
  errorMessage: string | null;
}) {
  if (!banner) return null;
  if (banner === "error") {
    return (
      <Banner tone="error">
        {errorMessage ?? "Sign-in failed. Please try again."}
      </Banner>
    );
  }
  if (banner === "verified") {
    return (
      <Banner tone="success" icon={<CheckCircle2 className="size-4" />}>
        Email verified. Sign in to continue.
      </Banner>
    );
  }
  if (banner === "reset") {
    return (
      <Banner tone="success" icon={<CheckCircle2 className="size-4" />}>
        Password updated. Sign in with your new password.
      </Banner>
    );
  }
  if (banner === "pending") {
    return (
      <Banner tone="info" icon={<Inbox className="size-4" />}>
        Check your email for a verification link.
      </Banner>
    );
  }
  return null;
}

function Banner({
  tone,
  icon,
  children,
}: {
  tone: "success" | "info" | "error";
  icon?: React.ReactNode;
  children: React.ReactNode;
}) {
  const toneClass =
    tone === "success"
      ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-300"
      : tone === "info"
        ? "border-sky-500/40 bg-sky-500/10 text-sky-200"
        : "border-destructive/40 bg-destructive/10 text-destructive";
  return (
    <div
      role={tone === "error" ? "alert" : "status"}
      className={`flex items-start gap-2 rounded-md border px-3 py-2 text-sm ${toneClass}`}
    >
      {icon}
      <span>{children}</span>
    </div>
  );
}

function SsoButton({ next }: { next: string | null }) {
  const href = next
    ? `/api/auth/oidc/start?redirect_after=${encodeURIComponent(next)}`
    : `/api/auth/oidc/start`;
  return (
    <Button asChild variant="outline" className="w-full">
      <Link href={href}>
        <LogIn className="size-4" />
        Sign in with SSO
      </Link>
    </Button>
  );
}

// Auth forms (this LoginForm, RegisterForm below, plus
// forgot-password and reset-password) use inline error banners +
// <FormMessage> instead of toasts; success is signalled by route
// navigation. See docs/dev/notifications-audit.md §F-6 for the
// standard. One comment covers both forms in this file.
function LoginForm({ next }: { next: string | null }) {
  const form = useForm<LoginValues>({
    resolver: zodResolver(loginSchema),
    defaultValues: { email: "", password: "" },
  });
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const onSubmit = form.handleSubmit(async (values) => {
    setError(null);
    setSubmitting(true);
    try {
      const res = await fetch("/api/auth/local/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "include",
        body: JSON.stringify(values),
      });
      if (!res.ok) {
        const msg = await readErrorMessage(res);
        setError(msg);
        return;
      }
      router.push(next ?? "/");
      router.refresh();
    } finally {
      setSubmitting(false);
    }
  });

  return (
    <Form {...form}>
      {/*
        Progressive enhancement: `method="POST"` + a real `action` URL
        means the form remains functional even when JS hasn't hydrated.
        Pre-M9 this form had neither — pressing Enter before hydration
        completed fell through to the browser's default GET handler and
        leaked `?email=&password=` into the URL bar, history, Referer,
        and the server access log. The handler at `/api/auth/local/login`
        accepts both JSON (XHR happy path below) and form-encoded
        (no-JS fallback) bodies and returns a 303 on the form path.
      */}
      <form
        onSubmit={onSubmit}
        method="POST"
        action="/api/auth/local/login"
        className="space-y-4"
      >
        {next ? <input type="hidden" name="next" value={next} /> : null}
        <EmailField form={form} />
        <FormField
          control={form.control}
          name="password"
          render={({ field }) => (
            <FormItem>
              <FormLabel>Password</FormLabel>
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
        {error ? <Banner tone="error">{error}</Banner> : null}
        <Button type="submit" disabled={submitting} className="w-full">
          {submitting ? "Signing in…" : "Sign in"}
        </Button>
      </form>
    </Form>
  );
}

function RegisterForm({
  next,
  onPending,
}: {
  next: string | null;
  onPending: (email: string) => void;
}) {
  const form = useForm<RegisterValues>({
    resolver: zodResolver(registerSchema),
    defaultValues: { email: "", password: "" },
  });
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const onSubmit = form.handleSubmit(async (values) => {
    setError(null);
    setSubmitting(true);
    try {
      const res = await fetch("/api/auth/local/register", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "include",
        body: JSON.stringify(values),
      });
      if (res.status === 202) {
        onPending(values.email);
        return;
      }
      if (!res.ok) {
        const msg = await readErrorMessage(res);
        setError(msg);
        return;
      }
      router.push(next ?? "/");
      router.refresh();
    } finally {
      setSubmitting(false);
    }
  });

  return (
    <Form {...form}>
      {/* Progressive enhancement — see LoginForm for rationale. */}
      <form
        onSubmit={onSubmit}
        method="POST"
        action="/api/auth/local/register"
        className="space-y-4"
      >
        {next ? <input type="hidden" name="next" value={next} /> : null}
        <EmailField form={form} />
        <FormField
          control={form.control}
          name="password"
          render={({ field }) => (
            <FormItem>
              <FormLabel>Password</FormLabel>
              <FormControl>
                <Input {...field} type="password" autoComplete="new-password" />
              </FormControl>
              <p className="text-muted-foreground text-xs">
                Must be at least 12 characters.
              </p>
              <FormMessage />
            </FormItem>
          )}
        />
        {error ? <Banner tone="error">{error}</Banner> : null}
        <Button type="submit" disabled={submitting} className="w-full">
          {submitting ? "Creating account…" : "Create account"}
        </Button>
      </form>
    </Form>
  );
}

function EmailField<T extends { email: string }>({
  form,
}: {
  form: ReturnType<typeof useForm<T>>;
}) {
  // react-hook-form's typing here is conservative; we know `email` is the
  // shared field across both schemas, so cast to the registered name.
  const name = "email" as never;
  return (
    <FormField
      control={form.control}
      name={name}
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
  );
}

function PendingVerificationView({
  email,
  onBack,
}: {
  email: string;
  onBack: () => void;
}) {
  const [resending, setResending] = useState(false);
  const [resent, setResent] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Auto-clear the "resent" badge after a few seconds.
  useEffect(() => {
    if (!resent) return;
    const t = setTimeout(() => setResent(false), 4000);
    return () => clearTimeout(t);
  }, [resent]);

  async function resend() {
    setError(null);
    setResending(true);
    try {
      await fetch("/api/auth/local/resend-verification", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        credentials: "include",
        body: JSON.stringify({ email }),
      });
      setResent(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not resend");
    } finally {
      setResending(false);
    }
  }

  return (
    <>
      <CardHeader className="space-y-2 text-center">
        <Mail className="text-muted-foreground mx-auto size-8" />
        <CardTitle className="text-xl">Check your email</CardTitle>
        <CardDescription>
          We sent a verification link to <strong>{email}</strong>. Click the
          link to activate your account, then sign in.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {resent ? (
          <Banner tone="success" icon={<CheckCircle2 className="size-4" />}>
            Verification email resent.
          </Banner>
        ) : null}
        {error ? <Banner tone="error">{error}</Banner> : null}
        <Button
          type="button"
          variant="outline"
          className="w-full"
          onClick={resend}
          disabled={resending}
        >
          {resending ? "Resending…" : "Resend verification email"}
        </Button>
      </CardContent>
      <CardFooter className="justify-center pt-0">
        <button
          type="button"
          onClick={onBack}
          className="text-muted-foreground hover:text-foreground text-xs underline-offset-4 hover:underline"
        >
          Back to sign-in
        </button>
      </CardFooter>
    </>
  );
}

async function readErrorMessage(res: Response): Promise<string> {
  try {
    const body = await res.json();
    return body?.error?.message ?? `HTTP ${res.status}`;
  } catch {
    return `HTTP ${res.status}`;
  }
}
