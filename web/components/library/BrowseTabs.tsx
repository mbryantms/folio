"use client";

import { usePathname, useRouter } from "next/navigation";
import * as React from "react";

import { PageHeader } from "@/components/admin/PageHeader";
import { CreatorsIndex } from "@/components/library/CreatorsIndex";
import { EntityIndex } from "@/components/library/EntityIndex";
import {
  BROWSE_TABS,
  parseBrowseTab,
  type BrowseTab,
} from "@/components/library/browse-tabs";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ENTITY_KINDS } from "@/lib/entities";

function tabLabel(tab: BrowseTab): string {
  return tab === "creators" ? "Creators" : ENTITY_KINDS[tab].plural;
}

/** The single "Browse" sidebar destination: one tabbed index over every
 *  browsable entity kind. The active tab lives in `?tab=` so it's
 *  shareable and survives reload; only the active tab is mounted (Radix
 *  unmounts inactive content), so switching tabs doesn't fire the other
 *  four list queries. The per-kind `/characters` etc. pages remain for
 *  deep links. */
export function BrowseTabs({
  initialTab,
  initialStartsWith,
}: {
  initialTab: BrowseTab;
  initialStartsWith?: string | null;
}) {
  const router = useRouter();
  const pathname = usePathname();
  const [tab, setTab] = React.useState<BrowseTab>(initialTab);
  // The server-parsed letter only applies to the tab the URL opened on.
  const startsWithFor = (t: BrowseTab) =>
    t === initialTab ? (initialStartsWith ?? null) : null;

  const onChange = (next: string) => {
    const t = parseBrowseTab(next);
    setTab(t);
    router.replace(`${pathname}?tab=${t}`, { scroll: false });
  };

  return (
    <div className="space-y-6">
      <PageHeader
        title="Browse"
        description="Characters, teams, story arcs, publishers, and creators across your libraries"
      />
      <Tabs value={tab} onValueChange={onChange}>
        <TabsList className="flex-wrap">
          {BROWSE_TABS.map((t) => (
            <TabsTrigger key={t} value={t}>
              {tabLabel(t)}
            </TabsTrigger>
          ))}
        </TabsList>
        {BROWSE_TABS.map((t) => (
          <TabsContent key={t} value={t} className="pt-4">
            {t === "creators" ? (
              <CreatorsIndex embedded initialStartsWith={startsWithFor(t)} />
            ) : (
              <EntityIndex
                kind={t}
                embedded
                initialStartsWith={startsWithFor(t)}
              />
            )}
          </TabsContent>
        ))}
      </Tabs>
    </div>
  );
}
