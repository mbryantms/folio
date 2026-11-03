/**
 * First-run tour steps. Each step spotlights a `[data-tour="…"]` anchor in
 * the library shell on Home; a step whose anchor isn't on screen (phone
 * layout, collapsed sidebar section, a renamed or hidden nav entry) shows
 * as a centered card instead, so the tour never dead-ends. The last step
 * is the starter reading-list shelf.
 */
export type TourStep = {
  id: string;
  /** `data-tour` values to spotlight, first visible wins (desktop shell
   *  first, then the phone bottom bar); omitted = centered card. */
  anchors?: string[];
  title: string;
  body: string;
  /** Renders the starter-list shelf instead of plain text. */
  kind?: "starter-lists";
};

export const TOUR_STEPS: TourStep[] = [
  {
    id: "welcome",
    title: "Welcome to Folio",
    body: "A quick look around takes about a minute. At the end you can pick a reading list to start with. Press Esc any time to skip.",
  },
  {
    id: "search",
    anchors: ["search", "tab-search"],
    title: "Search everything",
    body: "Find series, issues, creators, characters and your own notes from here. On a keyboard, press Ctrl K from anywhere.",
  },
  {
    id: "libraries",
    anchors: ["nav-libraries", "tab-library"],
    title: "Your libraries",
    body: "Each library on this server has its own shelf. All Libraries shows everything you can read in one place.",
  },
  {
    id: "home",
    anchors: ["home-rails"],
    title: "Home is yours to arrange",
    body: "Rails pick up where you left off and surface what's next. Pin any view, reading list or collection here from its ⋯ menu.",
  },
  {
    id: "collections",
    anchors: ["nav-collections", "tab-more"],
    title: "Collections and Want to Read",
    body: "Collect series and single issues into your own shelves. Every cover's ⋯ menu adds it to one, or to Want to Read.",
  },
  {
    id: "bookmarks",
    anchors: ["nav-bookmarks", "tab-bookmarks"],
    title: "Bookmarks, notes and highlights",
    body: "Mark pages while you read. Everything you've marked collects here and is searchable.",
  },
  {
    id: "settings",
    anchors: ["user-menu", "tab-more"],
    title: "Make it yours",
    body: "Reading modes, theme, cover styles and keyboard shortcuts are under Settings. Take the tour again from this menu any time.",
  },
  {
    id: "starter-lists",
    kind: "starter-lists",
    title: "Pick a place to start",
    body: "Reading lists put comics in order. These are the community's usual answers to “where do I start?”, with how much of each this library already has.",
  },
];
