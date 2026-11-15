"use client";

import { CreatorInsights } from "@/components/library/EntityInsights";
import {
  EntityProfile,
  EntitySections,
} from "@/components/library/EntityProfile";
import { useRefreshCreatorProfile } from "@/lib/api/mutations";
import { useCreatorDetail, useMe } from "@/lib/api/queries";
import type { CreatorDetailView } from "@/lib/api/types";

/** The provider profile and the "In your library" statistics of a creator
 *  page (roadmap WP-9.10): the same two blocks the character, team,
 *  location and story-arc pages carry, above the role rails.
 *
 *  The page itself is server-rendered; this is the live part. The header
 *  payload is polled while a profile fetch is pending, so the block fills
 *  in without a reload, and an admin gets Refresh. */
export function CreatorProfile({
  detail: initial,
}: {
  detail: CreatorDetailView;
}) {
  const live = useCreatorDetail(initial.slug, initial);
  const detail = live.data ?? initial;
  const me = useMe();
  const isAdmin = me.data?.role === "admin";
  const refresh = useRefreshCreatorProfile(detail.slug);
  return (
    <>
      <EntityProfile
        kind="creators"
        detail={detail}
        isAdmin={isAdmin}
        onRefresh={() => refresh.mutate()}
        refreshing={refresh.isPending}
      />
      <CreatorInsights slug={detail.slug} />
    </>
  );
}

/** The wiki sections of a creator's profile. They go **below** the role
 *  rails, unlike on a character page: a creator's write-up is often one
 *  long unheaded text (an interview, a full biography) that opens whole,
 *  and the series the creator worked on are what the page is for. Reads
 *  the same polled query as [`CreatorProfile`]. */
export function CreatorBiography({
  detail: initial,
}: {
  detail: CreatorDetailView;
}) {
  const live = useCreatorDetail(initial.slug, initial);
  const sections = (live.data ?? initial).profile?.sections ?? [];
  return sections.length > 0 ? <EntitySections sections={sections} /> : null;
}
