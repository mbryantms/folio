"use client";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { useAdminSettings, useEmailStatus } from "@/lib/api/queries";

import { EmailConfigForm } from "./EmailConfigForm";
import { EmailStatusCard } from "./EmailStatusCard";

/** Top-level shell for /admin/email. Loads the current `smtp.*` rows
 *  from `/admin/settings` and the operational probe from
 *  `/admin/email/status`. Renders a 2-up layout on lg+: config form on
 *  the left, status + test button on the right. */
export function EmailAdminClient() {
  const settings = useAdminSettings();
  const status = useEmailStatus();

  if (settings.isLoading || status.isLoading) {
    return (
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <Skeleton className="h-96 lg:col-span-2" />
        <Skeleton className="h-64" />
      </div>
    );
  }
  if (settings.error || !settings.data) {
    return (
      <p className="text-destructive text-sm">Failed to load email settings.</p>
    );
  }

  const initial = pickSmtpValues(settings.data.values);

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
      <Card className="lg:col-span-2">
        <CardHeader>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            SMTP configuration
          </CardTitle>
        </CardHeader>
        <CardContent>
          <EmailConfigForm initial={initial} />
        </CardContent>
      </Card>
      <EmailStatusCard
        status={status.data ?? null}
        canTest={Boolean(initial.host && initial.from)}
      />
    </div>
  );
}

export type SmtpInitial = {
  host: string;
  port: string;
  tls: "none" | "starttls" | "tls";
  username: string;
  /** True when a `smtp.password` row exists in the DB. The plaintext is
   *  never sent to the client; the form shows a placeholder when set and
   *  treats an empty input as "no change." */
  password_set: boolean;
  from: string;
};

function pickSmtpValues(
  rows: { key: string; value: unknown; is_secret: boolean }[],
): SmtpInitial {
  const asString = (k: string) => {
    const r = rows.find((x) => x.key === k);
    return typeof r?.value === "string" ? r.value : "";
  };
  const tls = asString("smtp.tls");
  const portRow = rows.find((x) => x.key === "smtp.port");
  return {
    host: asString("smtp.host"),
    port: typeof portRow?.value === "number" ? String(portRow.value) : "",
    tls:
      tls === "none" || tls === "starttls" || tls === "tls" ? tls : "starttls",
    username: asString("smtp.username"),
    password_set: rows.some(
      (r) => r.key === "smtp.password" && r.is_secret && r.value === "<set>",
    ),
    from: asString("smtp.from"),
  };
}
