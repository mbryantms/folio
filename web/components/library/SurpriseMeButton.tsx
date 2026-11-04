"use client";

import * as React from "react";
import { useRouter } from "next/navigation";
import { Sparkles } from "lucide-react";
import { toast } from "sonner";

import { Cover } from "@/components/Cover";
import { CoverMorph } from "@/components/library/CoverMorph";
import { Button } from "@/components/ui/button";
import { jsonFetch } from "@/lib/api/queries";
import type { SeriesListView } from "@/lib/api/types";
import { markCoverMorph, seriesCoverName } from "@/lib/ui/cover-morph";
import { cn } from "@/lib/utils";
import { seriesUrl } from "@/lib/urls";
import { useReducedMotion } from "@/lib/visual-effects";

type Pick = SeriesListView["items"][number];

/** How many random series the shuffle deals. */
const DEAL = 6;
/** One shuffle step; the deal runs ~600ms in all. */
const STEP_MS = 100;
/** Longest wait for the dealt covers to decode before shuffling anyway. */
const PRELOAD_CAP_MS = 400;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function preload(src: string): Promise<void> {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = img.onerror = () => resolve();
    img.src = src;
  });
}

/**
 * "Surprise me" — jumps to a random series detail page (audit 3.7
 * discovery). Backed by the server's `sort=random` (`ORDER BY random()`),
 * so the pick is unbiased across the whole accessible library rather than
 * a client-side sample. Lives in the Recently Added rail header on Home.
 *
 * Folio-flair: it deals a few random covers over the button, shuffles
 * through them (~600ms), lands on the pick, and the pick's cover morphs
 * into the series page (lib/ui/cover-morph.ts). Reduced motion, or a pick
 * without a cover: straight to the page.
 */
export function SurpriseMeButton({ className }: { className?: string }) {
  const router = useRouter();
  const reduced = useReducedMotion();
  const token = React.useId();
  const [busy, setBusy] = React.useState(false);
  const [deck, setDeck] = React.useState<Pick[] | null>(null);
  const [step, setStep] = React.useState(0);
  const alive = React.useRef(true);
  React.useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  async function surpriseMe() {
    if (busy) return;
    setBusy(true);
    try {
      const res = await jsonFetch<SeriesListView>(
        `/series?sort=random&limit=${DEAL}`,
      );
      const pick = res.items[0];
      if (!pick) {
        toast.info("No series to surprise you with yet.");
        return;
      }
      if (!reduced && pick.cover_url) {
        // Deal: the others first, the pick last (it ends on top).
        const others = res.items.slice(1).filter((s) => s.cover_url);
        const cards = [...others, pick];
        await Promise.race([
          Promise.all(cards.map((s) => preload(s.cover_url!))),
          sleep(PRELOAD_CAP_MS),
        ]);
        if (!alive.current) return;
        setStep(0);
        setDeck(cards);
        for (let i = 0; i < cards.length; i++) {
          setStep(i);
          await sleep(STEP_MS);
          if (!alive.current) return;
        }
        await sleep(STEP_MS);
        if (!alive.current) return;
      }
      // The pick's cover (top of the deal, under the same name) flies into
      // the series page's header; the loading skeleton holds it meanwhile.
      markCoverMorph(seriesCoverName(pick.slug), pick.cover_url, token);
      router.push(seriesUrl(pick));
      // A slow navigation, or one that stays here: put the deck away.
      setTimeout(() => {
        if (alive.current) setDeck(null);
      }, 2500);
    } catch {
      toast.error("Couldn't pick a series — try again.");
      setDeck(null);
    } finally {
      if (alive.current) setBusy(false);
    }
  }

  return (
    <span className={cn("relative inline-flex", className)}>
      <Button
        type="button"
        variant="ghost"
        size="sm"
        onClick={surpriseMe}
        disabled={busy}
        className="text-muted-foreground hover:text-foreground"
      >
        <Sparkles
          aria-hidden="true"
          className={cn("mr-1 size-3.5", busy && "surprise-sparkle")}
        />
        Surprise me
      </Button>
      {deck ? (
        <span
          aria-hidden="true"
          className="pointer-events-none absolute right-2 bottom-full z-20 mb-2 h-24 w-16"
        >
          {deck.map((s, i) => {
            // Dealt cards fan out behind the current one; the rest wait,
            // hidden, on the button.
            const offset = i - step;
            const style: React.CSSProperties =
              offset <= 0
                ? {
                    zIndex: i,
                    translate: `${offset * 7}px ${offset * -2}px`,
                    rotate: `${offset * 6}deg`,
                    opacity: offset < -3 ? 0 : 1,
                    scale: offset === 0 ? "1" : "0.94",
                  }
                : { zIndex: i, translate: "0 24px", opacity: 0, scale: "0.6" };
            const card = (
              <span
                className="surprise-card bg-muted absolute inset-0 overflow-hidden rounded-md shadow-lg ring-1 ring-black/10"
                style={style}
              >
                <Cover src={s.cover_url} alt="" className="size-full" />
              </span>
            );
            return i === deck.length - 1 ? (
              <CoverMorph key={s.id} name={seriesCoverName(s.slug)}>
                {card}
              </CoverMorph>
            ) : (
              <React.Fragment key={s.id}>{card}</React.Fragment>
            );
          })}
        </span>
      ) : null}
    </span>
  );
}
