"use client";

import { Check, X, Mail, AlertCircle } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useSendTestEmail } from "@/lib/api/mutations";
import type { EmailStatusView } from "@/lib/api/types";

export function EmailStatusCard({
  status,
  canTest,
}: {
  status: EmailStatusView | null;
  canTest: boolean;
}) {
  const test = useSendTestEmail();

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-muted-foreground flex items-center gap-2 text-sm font-semibold tracking-wide uppercase">
          <Mail className="h-4 w-4" />
          Status
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div>
          <Label>SMTP</Label>
          {status?.configured ? (
            <Badge variant="secondary" className="bg-emerald-500/15 text-emerald-300">
              Configured
            </Badge>
          ) : (
            <Badge variant="outline" className="border-amber-500/50 text-amber-300">
              Not wired
            </Badge>
          )}
        </div>

        {status && (
          <>
            <div>
              <Label>Last send</Label>
              <p className="text-foreground text-sm">
                {status.last_send_at
                  ? new Date(status.last_send_at).toLocaleString()
                  : "Never"}
              </p>
            </div>

            <div>
              <Label>Last result</Label>
              <ResultBadge ok={status.last_send_ok} />
              {status.last_duration_ms != null && (
                <span className="text-muted-foreground ml-2 text-xs">
                  {status.last_duration_ms} ms
                </span>
              )}
            </div>

            {status.last_error && (
              <div className="flex items-start gap-2 rounded-md border border-red-500/30 bg-red-500/5 p-2 text-xs text-red-300">
                <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                <span className="font-mono">{status.last_error}</span>
              </div>
            )}
          </>
        )}

        <Button
          onClick={() => test.mutate()}
          disabled={!canTest || test.isPending}
          className="w-full"
          variant="outline"
        >
          {test.isPending ? "Sending…" : "Send test email"}
        </Button>
        {!canTest && (
          <p className="text-muted-foreground text-xs">
            Save host + from address before testing.
          </p>
        )}
        {test.data?.delivered && (
          <p className="text-xs text-emerald-300">
            Delivered to {test.data.to} in {test.data.duration_ms} ms.
          </p>
        )}
      </CardContent>
    </Card>
  );
}

function Label({ children }: { children: React.ReactNode }) {
  return (
    <p className="text-muted-foreground mb-1 text-xs font-semibold tracking-wide uppercase">
      {children}
    </p>
  );
}

function ResultBadge({ ok }: { ok: boolean | null }) {
  if (ok == null)
    return (
      <Badge variant="outline" className="text-muted-foreground">
        —
      </Badge>
    );
  if (ok)
    return (
      <Badge variant="secondary" className="bg-emerald-500/15 text-emerald-300">
        <Check className="mr-1 h-3 w-3" />
        Delivered
      </Badge>
    );
  return (
    <Badge variant="secondary" className="bg-red-500/15 text-red-300">
      <X className="mr-1 h-3 w-3" />
      Failed
    </Badge>
  );
}
