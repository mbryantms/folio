import { ReadingListsIndex } from "@/components/cbl/ReadingListsIndex";

/** Reading lists — the main-UI sidebar "Reading lists" entry lands here.
 *  Own and shared CBL lists; each card opens the list at `/views/{id}`.
 *  The same board is the Reading lists tab of `/settings/views`. */
export default function ReadingListsPage() {
  return <ReadingListsIndex />;
}
