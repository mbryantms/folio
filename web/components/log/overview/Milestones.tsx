"use client";

import { BookOpen, Check, Flame, Lock, Star } from "lucide-react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogDescription,
  DialogPortal,
  DialogTitle,
} from "@/components/ui/dialog";
import type { Milestone, MilestoneIcon } from "@/lib/log/overview";
import { cn } from "@/lib/utils";
import { useReducedMotion } from "@/lib/visual-effects";

const ICONS: Record<MilestoneIcon, typeof Flame> = {
  flame: Flame,
  check: Check,
  book: BookOpen,
  star: Star,
};

/**
 * Milestones on the reading log's Overview (folio-flair phase 4). Earned
 * ones show their badge and a Replay button that plays the celebration
 * again; locked ones show how close you are.
 */
export function Milestones({
  items,
  coversFor,
}: {
  items: readonly Milestone[];
  /** Covers that illustrate a milestone (its reading window / series). */
  coversFor: (m: Milestone) => string[];
}) {
  const [playing, setPlaying] = useState<Milestone | null>(null);
  return (
    <>
      <ul className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {items.map((m) => {
          const Icon = m.earned ? ICONS[m.icon] : Lock;
          return (
            <li
              key={m.id}
              className={cn(
                "bg-card flex items-center gap-3 rounded-xl border p-4",
                !m.earned && "bg-card/60",
              )}
            >
              <span
                aria-hidden
                className={cn(
                  "grid size-11 shrink-0 place-items-center rounded-full",
                  m.earned
                    ? "bg-primary text-primary-foreground shadow-[inset_0_-3px_0_rgb(0_0_0/0.18),0_6px_16px_-6px_var(--primary)]"
                    : "bg-muted text-muted-foreground",
                )}
              >
                <Icon className="size-5" strokeWidth={2.25} />
              </span>
              <div className="min-w-0 flex-1">
                <p
                  className={cn(
                    "truncate text-sm font-semibold",
                    !m.earned && "text-muted-foreground",
                  )}
                  title={m.title}
                >
                  {m.title}
                </p>
                <p className="text-muted-foreground truncate text-xs">
                  {m.sub}
                </p>
                {!m.earned ? (
                  <div
                    className="bg-muted mt-2 h-1.5 overflow-hidden rounded-full"
                    role="progressbar"
                    aria-label={`${m.title} progress`}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={Math.round((m.progress ?? 0) * 100)}
                  >
                    <div
                      className="bg-primary/70 h-full rounded-full"
                      style={{
                        width: `${Math.round(Math.min(1, Math.max(0, m.progress ?? 0)) * 100)}%`,
                      }}
                    />
                  </div>
                ) : null}
              </div>
              {m.earned ? (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setPlaying(m)}
                  aria-label={`Replay ${m.title}`}
                >
                  Replay
                </Button>
              ) : null}
            </li>
          );
        })}
      </ul>
      {playing ? (
        <Celebration
          title={playing.title}
          sub={playing.sub}
          covers={coversFor(playing)}
          onClose={() => setPlaying(null)}
        />
      ) : null}
    </>
  );
}

/**
 * A milestone celebration: covers burst out from the centre around a card
 * with the milestone. Built on the Radix dialog, so focus stays inside,
 * Esc closes it and focus goes back to what opened it; the button or a
 * click outside the card closes it too. The full-screen content is the
 * dialog itself so the burst can fly across the whole viewport.
 * Reduced motion: the card alone, no burst.
 */
export function Celebration({
  title,
  sub,
  covers,
  onClose,
}: {
  title: string;
  sub: string;
  covers: readonly string[];
  onClose: () => void;
}) {
  const reduced = useReducedMotion();
  const buttonRef = useRef<HTMLButtonElement>(null);
  const [leaving, setLeaving] = useState(false);
  // What had focus when it opened (the Replay button): the dialog has no
  // Radix trigger to send focus back to, so it is restored by hand.
  const [opener] = useState(() =>
    typeof document === "undefined"
      ? null
      : (document.activeElement as HTMLElement | null),
  );
  const close = () => {
    if (leaving) return;
    if (reduced) return onClose();
    setLeaving(true);
    window.setTimeout(onClose, 220);
  };

  const burst = reduced ? [] : covers.slice(0, 8);
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
    >
      <DialogPortal>
        <DialogPrimitive.Content
          className={cn(
            "celebrate fixed inset-0 z-[60] grid place-items-center bg-black/55 p-4 backdrop-blur-sm outline-none",
            leaving && "is-leaving",
          )}
          onOpenAutoFocus={(e) => {
            e.preventDefault();
            buttonRef.current?.focus();
          }}
          onCloseAutoFocus={(e) => {
            e.preventDefault();
            if (opener?.isConnected) opener.focus();
          }}
        >
          {/* The backdrop is a real (unfocusable) button: a click outside the
              card closes it; keyboard users have Esc and the Nice button. */}
          <button
            type="button"
            tabIndex={-1}
            aria-label="Close"
            className="absolute inset-0 cursor-default"
            onClick={close}
          />
          <div aria-hidden className="pointer-events-none absolute inset-0">
            {burst.map((src, i) => {
              const a = (i / burst.length) * Math.PI * 2 - Math.PI / 2;
              return (
                // eslint-disable-next-line @next/next/no-img-element
                <img
                  key={`${src}-${i}`}
                  src={src}
                  alt=""
                  className="celebrate-cover absolute top-1/2 left-1/2 aspect-[2/3] w-20 rounded-md object-cover shadow-xl sm:w-24"
                  style={
                    {
                      "--dx": `${Math.cos(a) * 230}px`,
                      "--dy": `${Math.sin(a) * 170}px`,
                      "--r": `${(i % 2 ? 1 : -1) * (6 + i * 3)}deg`,
                      animationDelay: `${i * 40}ms`,
                    } as React.CSSProperties
                  }
                />
              );
            })}
          </div>
          <div className="celebrate-card bg-card relative w-full max-w-xs rounded-2xl border p-6 text-center shadow-2xl">
            <p className="text-primary text-xs font-bold tracking-[0.08em] uppercase">
              Milestone
            </p>
            <DialogTitle className="mt-1 text-xl font-semibold text-balance">
              {title}
            </DialogTitle>
            <DialogDescription className="mt-1">{sub}</DialogDescription>
            <Button ref={buttonRef} className="mt-5 w-full" onClick={close}>
              Nice
            </Button>
          </div>
        </DialogPrimitive.Content>
      </DialogPortal>
    </Dialog>
  );
}
