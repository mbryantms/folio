"use client";

/**
 * `<ProvidersTab>` — /admin/metadata `?tab=providers` (M6).
 *
 * Per-provider connectivity + quota + "Test" button. The actual
 * credential / enabled-toggle inputs live on `/admin/settings` (the
 * unified settings page that surfaces every `metadata.*` key); this
 * tab links to that surface with a deep-link hash so the operator
 * doesn't have to drill.
 */

import { CheckCircle2, ExternalLink, Loader2, XCircle } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useTestMetadataProvider } from "@/lib/api/mutations";
import { useAdminMetadataProviders } from "@/lib/api/queries";
import type { ProviderView } from "@/lib/api/types";

import { ProviderConfigForm } from "./ProviderConfigForm";

export function ProvidersTab() {
  const q = useAdminMetadataProviders();
  if (q.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-6 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading…
      </div>
    );
  }
  return (
    <div className="space-y-3">
      {(q.data?.providers ?? []).map((p) => (
        <ProviderCard key={p.id} provider={p} />
      ))}
      <p className="text-muted-foreground text-xs">
        Cache TTLs, auto-apply threshold, and other operational tuning
        live on the generic Server settings page (search for{" "}
        <code>metadata.*</code>). The cards above own the credentials
        + enable toggles.
      </p>
    </div>
  );
}

function ProviderCard({ provider }: { provider: ProviderView }) {
  const test = useTestMetadataProvider();
  const onTest = () => test.mutate({ id: provider.id });
  const lastResult = test.data?.ok;
  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between pb-2">
        <CardTitle className="flex items-center gap-2 text-sm font-medium">
          {provider.label}
          {provider.enabled ? (
            <Badge variant="default" className="text-[10px]">
              ENABLED
            </Badge>
          ) : provider.configured ? (
            <Badge variant="outline" className="text-[10px]">
              DISABLED
            </Badge>
          ) : (
            <Badge variant="secondary" className="text-[10px]">
              NOT CONFIGURED
            </Badge>
          )}
        </CardTitle>
        <Button
          size="sm"
          variant="outline"
          onClick={onTest}
          disabled={test.isPending || !provider.enabled}
        >
          {test.isPending ? (
            <>
              <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Testing
            </>
          ) : (
            "Test"
          )}
        </Button>
      </CardHeader>
      <CardContent className="space-y-2 text-sm">
        <div className="text-muted-foreground flex items-center gap-3">
          {provider.quota ? (
            <>
              {provider.quota.remaining_hour != null && (
                <span>
                  {provider.quota.remaining_hour.toLocaleString()} /hr
                </span>
              )}
              {provider.quota.remaining_day != null && (
                <span>
                  · {provider.quota.remaining_day.toLocaleString()} /day
                </span>
              )}
            </>
          ) : (
            <span>No quota data</span>
          )}
        </div>
        {test.error && (
          <div className="text-destructive flex items-center gap-1 text-xs">
            <XCircle className="h-3 w-3" /> {test.error.message}
          </div>
        )}
        {!test.error && lastResult === true && (
          <div className="text-xs text-emerald-700 dark:text-emerald-300">
            <CheckCircle2 className="mr-1 inline h-3 w-3" /> Live —{" "}
            {test.data?.duration_ms}ms round-trip.
          </div>
        )}
        <a
          href={
            provider.id === "comicvine"
              ? "https://comicvine.gamespot.com/api/"
              : "https://metron.cloud/"
          }
          target="_blank"
          rel="noreferrer"
          className="text-muted-foreground inline-flex items-center gap-1 text-xs hover:underline"
        >
          Provider docs <ExternalLink className="h-3 w-3" />
        </a>
        {(provider.id === "comicvine" || provider.id === "metron") && (
          <ProviderConfigForm
            provider={provider.id as "comicvine" | "metron"}
          />
        )}
      </CardContent>
    </Card>
  );
}
