/**
 * Variants for `/pwa-lab` — a throwaway diagnostic surface for the iOS /
 * iPadOS 26 status-bar fade in the installed app (see #994, #995, #999 and
 * its revert #1003). Each variant isolates one hypothesis about what the
 * system draws into the status-bar strip. Not linked from the app; delete
 * the route once the fade is understood.
 */
export interface LabVariant {
  id: string;
  letter: string;
  title: string;
  question: string;
}

export const LAB_VARIANTS: readonly LabVariant[] = [
  {
    id: "a-baseline",
    letter: "A",
    title: "Page scroll, colour to the top",
    question: "Baseline — should reproduce the fade.",
  },
  {
    id: "b-fixed-black",
    letter: "B",
    title: "Page scroll + fixed 56px black bar",
    question: "Does a solid fixed bar get extended into the status-bar strip?",
  },
  {
    id: "c-inner-scroll",
    letter: "C",
    title: "Page never scrolls; full-screen inner scroller",
    question:
      "Does the strip only show content that scrolls under it at document level?",
  },
  {
    id: "d-inner-scroll-black-top",
    letter: "D",
    title: "Inner scroller below a static 56px black block",
    question: "C plus a solid top edge.",
  },
  {
    id: "e-static-black-top",
    letter: "E",
    title: "Page scroll, tall static black block first",
    question:
      "Check at rest (strip should be black), then scroll — does colour appear live?",
  },
  {
    id: "f-inner-scroll-12px",
    letter: "F",
    title: "Inner scroller below a 12px black band",
    question: "Smallest band WebKit should treat as a solid edge (> 10px).",
  },
  {
    id: "g-inner-scroll-4px",
    letter: "G",
    title: "Inner scroller below a 4px black band",
    question: "Below WebKit's 10px floor — expected to fade like C.",
  },
  {
    id: "h-app-header",
    letter: "H",
    title: "Inner scroller below a 56px header in the app colour",
    question: "Preview of the library: strip should match the header, no fade.",
  },
  {
    id: "i-reader-mock",
    letter: "I",
    title: "Reader mock: 12px band + tap to toggle 56px chrome",
    question: "Both reader states: chrome hidden (thin band) and shown.",
  },
] as const;

export function labVariant(id: string): LabVariant | undefined {
  return LAB_VARIANTS.find((v) => v.id === id);
}
