import { MarkersList } from "@/components/markers/MarkersList";

/** Global feed of every marker the calling user has created (bookmarks,
 *  notes, favorites, highlights — all kinds share this index). The
 *  sidebar label stays "Bookmarks" because that's the user-facing
 *  primary kind, but the page itself shows every kind with a filter
 *  chip row. The list is a client component so the filter + debounced
 *  search state lives next to the TanStack Query cache. */
export default function BookmarksIndexPage() {
  return <MarkersList />;
}
