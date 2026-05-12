"use client";

import * as React from "react";

import { PageHeader } from "@/components/admin/PageHeader";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useUser } from "@/lib/api/queries";
import { AuditTable } from "./AuditTable";
import { LibraryAccessMatrix } from "./LibraryAccessMatrix";
import { UserProfileForm } from "./UserProfileForm";
import { UserReadingStats } from "./UserReadingStats";

export function UserDetail({ id }: { id: string }) {
  const { data, isLoading, error } = useUser(id);
  const [tab, setTab] = React.useState<string>("profile");

  if (isLoading) {
    return (
      <>
        <PageHeader title="Loading…" />
        <Skeleton className="h-64 w-full" />
      </>
    );
  }
  if (error) {
    return (
      <>
        <PageHeader title="User" />
        <p className="text-destructive text-sm">{error.message}</p>
      </>
    );
  }
  if (!data) return null;

  return (
    <>
      <PageHeader
        title={data.display_name}
        description={data.email ?? "No email on file"}
      />
      <Tabs value={tab} onValueChange={setTab} className="w-full">
        <TabsList>
          <TabsTrigger value="profile">Profile</TabsTrigger>
          <TabsTrigger value="access">Library access</TabsTrigger>
          <TabsTrigger value="activity">Audit log</TabsTrigger>
          <TabsTrigger value="reading">Reading</TabsTrigger>
        </TabsList>
        <TabsContent value="profile">
          <UserProfileForm user={data} />
        </TabsContent>
        <TabsContent value="access">
          <LibraryAccessMatrix user={data} />
        </TabsContent>
        <TabsContent value="activity">
          <AuditTable pinnedActorId={data.id} />
        </TabsContent>
        <TabsContent value="reading">
          {/* Mount only when the tab is open so the audit row is written
              on user intent, not on page navigation. */}
          {tab === "reading" ? <UserReadingStats userId={data.id} /> : null}
        </TabsContent>
      </Tabs>
    </>
  );
}
