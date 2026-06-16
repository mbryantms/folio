"use client";

import Link from "next/link";
import { Info } from "lucide-react";

import { PageHeader } from "@/components/admin/PageHeader";
import { settingsNav } from "@/components/admin/nav";
import { navIcons } from "@/components/admin/nav-icons";
import { useMe } from "@/lib/api/queries";

/** `/settings` landing — a real destination (vs the old redirect to a leaf)
 *  that lays out every settings section as a card grid. Reuses the same
 *  `settingsNav()` data + icon map the sidebar renders, so the index and the
 *  sidebar never drift. Admins also get an "About" card to the server-info
 *  page (version / commit / update check). */
export function SettingsIndex() {
  const me = useMe();
  const isAdmin = me.data?.role === "admin";
  const sections = settingsNav("");

  return (
    <div className="space-y-8">
      <PageHeader
        title="Settings"
        description="Manage your reader preferences, saved views, pages, account, and access."
      />

      {sections.map((section) => (
        <section key={section.label} className="space-y-3">
          <h2 className="text-muted-foreground/70 text-[11px] font-medium tracking-widest uppercase">
            {section.label}
          </h2>
          <ul
            role="list"
            className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4"
          >
            {section.items.map((item) => {
              const Icon = navIcons[item.icon];
              return (
                <li key={item.href}>
                  <SettingsCard href={item.href} label={item.label}>
                    <Icon
                      className="text-muted-foreground h-5 w-5 shrink-0"
                      aria-hidden="true"
                    />
                  </SettingsCard>
                </li>
              );
            })}
          </ul>
        </section>
      ))}

      {isAdmin ? (
        <section className="space-y-3">
          <h2 className="text-muted-foreground/70 text-[11px] font-medium tracking-widest uppercase">
            About
          </h2>
          <ul
            role="list"
            className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4"
          >
            <li>
              <SettingsCard href="/admin/server" label="About Folio">
                <Info
                  className="text-muted-foreground h-5 w-5 shrink-0"
                  aria-hidden="true"
                />
              </SettingsCard>
            </li>
          </ul>
        </section>
      ) : null}
    </div>
  );
}

function SettingsCard({
  href,
  label,
  children,
}: {
  href: string;
  label: string;
  children: React.ReactNode;
}) {
  return (
    <Link
      href={href}
      className="group hover:bg-accent/40 focus-visible:ring-ring border-border/60 flex h-full items-center gap-3 rounded-lg border p-4 transition-colors focus-visible:ring-2 focus-visible:outline-none"
    >
      {children}
      <span className="truncate font-medium">{label}</span>
    </Link>
  );
}
