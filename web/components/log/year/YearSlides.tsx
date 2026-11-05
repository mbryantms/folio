"use client";

import { ArrowRight, Flame, RotateCcw } from "lucide-react";
import Link from "next/link";
import { useLayoutEffect, useRef } from "react";

import { CountUp } from "@/components/CountUp";
import { Cover } from "@/components/Cover";
import { Button } from "@/components/ui/button";
import { dayLabel } from "@/lib/log/overview";
import {
  WEEKDAYS,
  durationParts,
  hourLabel,
  runLabel,
  type SlideId,
} from "@/lib/log/year";
import { cn } from "@/lib/utils";

import type { YearModel } from "./YearStory";

type SlideProps = {
  model: YearModel;
  reduced: boolean;
  years: number[];
  onReplay: () => void;
  onPickYear: (year: number) => void;
};

/** One slide of the Year in review story. */
export function YearSlide({ id, ...props }: SlideProps & { id: SlideId }) {
  switch (id) {
    case "intro":
      return <Intro {...props} />;
    case "numbers":
      return <Numbers {...props} />;
    case "months":
      return <Months {...props} />;
    case "top":
      return <Top {...props} />;
    case "rhythm":
      return <When {...props} />;
    case "streak":
      return <Streak {...props} />;
    case "mosaic":
      return <Mosaic {...props} />;
    case "outro":
      return <Outro {...props} />;
    case "empty":
      return <Empty {...props} />;
  }
}

const fmt = (n: number) => n.toLocaleString("en-US");
const plural = (n: number, one: string, many = `${one}s`) =>
  n === 1 ? one : many;
const monthLong = (key: string) =>
  new Date(`${key}-01T00:00:00Z`).toLocaleString("en-US", {
    month: "long",
    timeZone: "UTC",
  });

/**
 * Entrance motion for a slide: `[data-enter]` children rise in one after
 * another, `[data-grow]` bars grow up and `[data-grow-x]` bars grow right.
 * Skipped entirely under reduced motion (and where WAAPI is missing).
 */
function useEnter(reduced: boolean) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || reduced || typeof el.animate !== "function") return;
    const ease = "cubic-bezier(0.2, 0.7, 0.2, 1)";
    const anims: Animation[] = [];
    el.querySelectorAll<HTMLElement>("[data-enter]").forEach((n, i) =>
      anims.push(
        n.animate(
          [
            { opacity: 0, transform: "translateY(14px)" },
            { opacity: 1, transform: "none" },
          ],
          { duration: 520, delay: i * 90, easing: ease, fill: "backwards" },
        ),
      ),
    );
    el.querySelectorAll<HTMLElement>("[data-grow]").forEach((n, i) =>
      anims.push(
        n.animate([{ transform: "scaleY(0)" }, { transform: "scaleY(1)" }], {
          duration: 650,
          delay: 250 + i * 45,
          easing: ease,
          fill: "backwards",
        }),
      ),
    );
    el.querySelectorAll<HTMLElement>("[data-grow-x]").forEach((n, i) =>
      anims.push(
        n.animate([{ transform: "scaleX(0)" }, { transform: "scaleX(1)" }], {
          duration: 650,
          delay: 300 + i * 80,
          easing: ease,
          fill: "backwards",
        }),
      ),
    );
    el.querySelectorAll<HTMLElement>("[data-pop]").forEach((n, i) =>
      anims.push(
        n.animate(
          [
            { opacity: 0, transform: "scale(0.85)" },
            { opacity: 1, transform: "none" },
          ],
          {
            duration: 420,
            delay: 200 + Math.min(i, 40) * 25,
            easing: ease,
            fill: "backwards",
          },
        ),
      ),
    );
    return () => anims.forEach((a) => a.cancel());
  }, [reduced]);
  return ref;
}

function Frame({
  reduced,
  children,
  className,
}: {
  reduced: boolean;
  children: React.ReactNode;
  className?: string;
}) {
  const ref = useEnter(reduced);
  return (
    <div
      ref={ref}
      className={cn(
        "flex h-full flex-col justify-center gap-6 overflow-hidden p-6 sm:p-10",
        className,
      )}
    >
      {children}
    </div>
  );
}

function Kicker({ children }: { children: React.ReactNode }) {
  return (
    <p
      data-enter
      className="text-xs font-semibold tracking-[0.12em] text-(--year-accent) uppercase"
    >
      {children}
    </p>
  );
}

function Headline({ children }: { children: React.ReactNode }) {
  return (
    <h2
      data-enter
      className="text-3xl leading-tight font-semibold tracking-tight text-balance sm:text-4xl"
    >
      {children}
    </h2>
  );
}

function Stat({
  label,
  value,
  unit,
}: {
  label: string;
  value: number;
  unit?: string;
}) {
  return (
    <div data-enter className="min-w-0">
      <p className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
        {label}
      </p>
      <p className="mt-1 truncate text-3xl font-semibold tracking-tight tabular-nums">
        <CountUp value={value} />
        {unit ? (
          <span className="text-muted-foreground ml-1 text-base font-normal">
            {unit}
          </span>
        ) : null}
      </p>
    </div>
  );
}

/** A fan of up to three covers. */
function CoverFan({ covers }: { covers: string[] }) {
  if (!covers.length) return null;
  const fan = covers.slice(0, 3);
  return (
    <div
      aria-hidden
      data-enter
      className="relative mx-auto h-36 w-56 sm:h-44 sm:w-64"
    >
      {fan.map((src, k) => {
        const o = fan.length === 1 ? 0 : k - (fan.length - 1) / 2;
        return (
          <div
            key={src}
            className="absolute bottom-0 left-1/2 w-24 shadow-xl sm:w-28"
            style={{
              translate: `calc(-50% + ${o * 58}px) ${Math.abs(o) * 10}px`,
              rotate: `${o * 10}deg`,
              zIndex: 3 - Math.abs(o),
            }}
          >
            <Cover src={src} alt="" />
          </div>
        );
      })}
    </div>
  );
}

function fanCovers(model: YearModel): string[] {
  const fromTop = model.top.flatMap((t) => (t.cover ? [t.cover] : []));
  return [
    ...new Set([...fromTop, ...model.mosaic.covers.map((c) => c.src)]),
  ].slice(0, 3);
}

function Intro({ model, reduced }: SlideProps) {
  const sofar = model.inProgress ? " so far" : "";
  return (
    <Frame reduced={reduced} className="items-center text-center">
      <CoverFan covers={fanCovers(model)} />
      <div>
        <p data-enter className="text-xl font-medium sm:text-2xl">
          Your
        </p>
        <p
          data-enter
          className="text-[5.5rem] leading-none font-bold tracking-tighter text-(--year-accent) tabular-nums sm:text-9xl"
        >
          {model.year}
        </p>
        <p data-enter className="text-xl font-medium sm:text-2xl">
          in comics
        </p>
      </div>
      <p data-enter className="text-muted-foreground text-sm">
        {fmt(model.daysActive)} reading {plural(model.daysActive, "day")}
        {model.issues
          ? ` · ${fmt(model.issues)} ${plural(model.issues, "issue")}`
          : ""}
        {sofar}
      </p>
      <p
        data-enter
        className="text-muted-foreground inline-flex items-center gap-1 text-xs"
      >
        Tap, swipe or use the arrow keys
        <ArrowRight className="size-3.5" aria-hidden />
      </p>
    </Frame>
  );
}

function Numbers({ model, reduced }: SlideProps) {
  const time = durationParts(model.activeMs);
  const perDay = model.daysActive
    ? Math.round(model.pages / model.daysActive)
    : 0;
  return (
    <Frame reduced={reduced}>
      <Kicker>Pages turned</Kicker>
      <div data-enter>
        <p className="text-7xl leading-none font-bold tracking-tighter text-(--year-accent) sm:text-8xl">
          <CountUp value={model.pages} />
        </p>
        <p className="text-muted-foreground mt-2 text-lg">
          {plural(model.pages, "page")} read in {model.year}
          {model.inProgress ? " so far" : ""}
        </p>
      </div>
      <div className="grid grid-cols-2 gap-x-4 gap-y-5 border-t pt-6">
        <Stat label="Time reading" value={time.value} unit={time.unit} />
        <Stat
          label="Issues read"
          value={model.issues}
          unit={plural(model.issues, "issue")}
        />
        <Stat
          label="Reading days"
          value={model.daysActive}
          unit={plural(model.daysActive, "day")}
        />
        <Stat
          label="Sessions"
          value={model.sessions}
          unit={plural(model.sessions, "session")}
        />
      </div>
      {perDay > 0 ? (
        <p data-enter className="text-muted-foreground text-sm">
          About{" "}
          <span className="text-foreground font-semibold">{fmt(perDay)}</span>{" "}
          pages on every day you read.
        </p>
      ) : null}
    </Frame>
  );
}

function Months({ model, reduced }: SlideProps) {
  const best = model.months[model.busiest]!;
  const max = Math.max(1, best.pages);
  const share = model.pages ? Math.round((best.pages / model.pages) * 100) : 0;
  return (
    <Frame reduced={reduced}>
      <Kicker>Busiest month</Kicker>
      <Headline>
        <span className="text-(--year-accent)">{monthLong(best.key)}</span> was
        your big one
      </Headline>
      <p data-enter className="text-muted-foreground -mt-3 text-sm">
        {fmt(best.pages)} pages{share ? ` · ${share}% of your year` : ""}
      </p>
      <ul
        role="list"
        aria-label="Pages per month"
        className="grid h-40 grid-cols-12 items-end gap-1 sm:h-52 sm:gap-1.5"
      >
        {model.months.map((m, i) => {
          const top = i === model.busiest;
          return (
            <li key={m.key} className="flex h-full flex-col justify-end gap-1">
              <span className="sr-only">
                {monthLong(m.key)}: {fmt(m.pages)} pages
              </span>
              <span
                aria-hidden
                data-grow
                className={cn(
                  "block w-full origin-bottom rounded-t-md",
                  top
                    ? "bg-primary"
                    : "bg-[color-mix(in_oklab,var(--primary)_28%,var(--muted))]",
                )}
                style={{
                  height: `${Math.max(m.pages ? 4 : 1.5, (m.pages / max) * 100)}%`,
                }}
              />
              <span
                aria-hidden
                className={cn(
                  "text-center text-xs",
                  top
                    ? "text-foreground font-semibold"
                    : "text-muted-foreground",
                )}
              >
                <span className="sm:hidden">{m.label[0]}</span>
                <span className="hidden sm:inline">{m.label}</span>
              </span>
            </li>
          );
        })}
      </ul>
      {model.bigDay ? (
        <p data-enter className="text-muted-foreground text-sm">
          Biggest day:{" "}
          <span className="text-foreground font-medium">
            {dayLabel(model.bigDay.date)}
          </span>{" "}
          · {fmt(model.bigDay.pages)} pages
        </p>
      ) : null}
    </Frame>
  );
}

function hours(ms: number) {
  const d = durationParts(ms);
  return `${fmt(d.value)} ${d.unit}`;
}

function Top({ model, reduced }: SlideProps) {
  const [first, ...rest] = model.top;
  if (!first) return null;
  return (
    <Frame reduced={reduced}>
      <Kicker>Most read</Kicker>
      <Headline>Your top series</Headline>
      <div data-enter className="flex items-center gap-4">
        <div className="w-24 shrink-0 shadow-lg sm:w-28">
          <Cover src={first.cover} alt="" fallback={first.name} />
        </div>
        <div className="min-w-0">
          <p className="text-xs font-semibold tracking-wide text-(--year-accent) uppercase">
            No. 1
          </p>
          <p className="line-clamp-2 text-xl font-semibold tracking-tight sm:text-2xl">
            {first.name}
          </p>
          <p className="text-muted-foreground mt-1 text-sm">
            {hours(first.active_ms)} · {fmt(first.sessions)}{" "}
            {plural(first.sessions, "session")}
            {first.finished ? ` · ${fmt(first.finished)} finished` : ""}
          </p>
        </div>
      </div>
      {rest.length ? (
        <ol className="divide-border divide-y border-t">
          {rest.map((s, i) => (
            <li
              key={s.series_id}
              data-enter
              className="flex items-center gap-3 py-2.5"
            >
              <span className="text-muted-foreground w-5 shrink-0 text-sm font-semibold tabular-nums">
                {i + 2}
              </span>
              <div className="w-9 shrink-0">
                <Cover src={s.cover} alt="" fallback={s.name} />
              </div>
              <p className="min-w-0 flex-1 truncate text-sm font-medium">
                {s.name}
              </p>
              <p className="text-muted-foreground shrink-0 text-xs tabular-nums">
                {hours(s.active_ms)}
              </p>
            </li>
          ))}
        </ol>
      ) : null}
    </Frame>
  );
}

const DOW_SHORT = ["M", "T", "W", "T", "F", "S", "S"];

function When({ model, reduced }: SlideProps) {
  const r = model.rhythm;
  if (!r) return null;
  const maxDow = Math.max(1, ...r.byDow);
  return (
    <Frame reduced={reduced}>
      <Kicker>When you read</Kicker>
      <Headline>
        You&apos;re <span className="text-(--year-accent)">{r.persona}</span>
      </Headline>
      <p data-enter className="text-muted-foreground -mt-3 text-sm">
        Most often on {WEEKDAYS[r.peakDow]}s, around {hourLabel(r.peakHour)}.
      </p>
      <ul
        role="list"
        aria-label="Reading by weekday"
        className="grid h-28 grid-cols-7 items-end gap-2 sm:h-32"
      >
        {r.byDow.map((v, i) => {
          const top = i === r.peakDow;
          return (
            <li
              key={WEEKDAYS[i]}
              className="flex h-full flex-col justify-end gap-1"
            >
              <span className="sr-only">
                {WEEKDAYS[i]}: {Math.round((v / maxDow) * 100)}% of your busiest
                day
              </span>
              <span
                aria-hidden
                data-grow
                className={cn(
                  "block w-full origin-bottom rounded-t-md",
                  top
                    ? "bg-primary"
                    : "bg-[color-mix(in_oklab,var(--primary)_28%,var(--muted))]",
                )}
                style={{ height: `${Math.max(3, (v / maxDow) * 100)}%` }}
              />
              <span
                aria-hidden
                className={cn(
                  "text-center text-xs",
                  top
                    ? "text-foreground font-semibold"
                    : "text-muted-foreground",
                )}
              >
                {DOW_SHORT[i]}
              </span>
            </li>
          );
        })}
      </ul>
      <ul role="list" aria-label="Reading by time of day" className="space-y-2">
        {r.byTime.map((t) => {
          const pct = Math.round(t.share * 100);
          const top = t.id === r.peakTime;
          return (
            <li
              key={t.id}
              className="grid grid-cols-[5.5rem_1fr_2.5rem] items-center gap-3"
            >
              <span
                className={cn(
                  "text-xs",
                  top ? "font-semibold" : "text-muted-foreground",
                )}
              >
                {t.label}
              </span>
              <span
                aria-hidden
                className="bg-muted block h-2 overflow-hidden rounded-full"
              >
                <span
                  data-grow-x
                  className={cn(
                    "block h-full origin-left rounded-full",
                    top
                      ? "bg-primary"
                      : "bg-[color-mix(in_oklab,var(--primary)_35%,var(--muted))]",
                  )}
                  style={{ width: `${pct}%` }}
                />
              </span>
              <span className="text-muted-foreground text-right text-xs tabular-nums">
                {pct}%
              </span>
            </li>
          );
        })}
      </ul>
    </Frame>
  );
}

function Streak({ model, reduced }: SlideProps) {
  const run = model.run;
  if (!run) return null;
  const shown = Math.min(run.length, 42);
  const share = model.daysInScope
    ? Math.min(1, model.daysActive / model.daysInScope)
    : 0;
  return (
    <Frame reduced={reduced}>
      <Kicker>Longest streak</Kicker>
      <div data-enter className="flex items-end gap-3">
        <p className="text-8xl leading-none font-bold tracking-tighter text-(--year-accent) sm:text-9xl">
          <CountUp value={run.length} />
        </p>
        <p className="pb-2 text-lg leading-tight font-medium">
          days
          <br />
          in a row
        </p>
      </div>
      <p data-enter className="text-muted-foreground -mt-2 text-sm">
        {runLabel(run)}
      </p>
      <div aria-hidden className="flex flex-wrap gap-1.5">
        {Array.from({ length: shown }, (_, i) => (
          <span
            key={i}
            data-pop
            className="bg-primary text-primary-foreground grid size-6 place-items-center rounded-md"
          >
            <Flame className="size-3.5" />
          </span>
        ))}
        {run.length > shown ? (
          <span
            data-pop
            className="text-muted-foreground grid h-6 place-items-center px-1 text-xs"
          >
            +{run.length - shown}
          </span>
        ) : null}
      </div>
      <div data-enter className="space-y-2 border-t pt-5">
        <p className="text-sm">
          You read on{" "}
          <span className="font-semibold tabular-nums">
            {fmt(model.daysActive)}
          </span>{" "}
          of {fmt(model.daysInScope)} days
          {model.inProgress ? " so far" : ""} this year.
        </p>
        <span
          aria-hidden
          className="bg-muted block h-2 overflow-hidden rounded-full"
        >
          <span
            data-grow-x
            className="bg-primary block h-full origin-left rounded-full"
            style={{ width: `${Math.round(share * 100)}%` }}
          />
        </span>
      </div>
    </Frame>
  );
}

function Mosaic({ model, reduced }: SlideProps) {
  const { covers, total, more } = model.mosaic;
  return (
    <Frame reduced={reduced} className="justify-start">
      <Kicker>Everything you finished</Kicker>
      <Headline>
        <span className="text-(--year-accent) tabular-nums">
          {model.coversLoading ? "…" : fmt(total)}
          {more ? "+" : ""}
        </span>{" "}
        {plural(total, "issue")} finished
      </Headline>
      <div className="relative min-h-0 flex-1 overflow-hidden [mask-image:linear-gradient(to_bottom,black_75%,transparent)]">
        {/* A wall of covers is decoration; the headline carries the count. */}
        <div aria-hidden className="grid grid-cols-5 gap-1.5 sm:grid-cols-6">
          {covers.map((c) => (
            <div key={c.key} data-pop title={c.title}>
              <Cover src={c.src} alt="" />
            </div>
          ))}
        </div>
      </div>
      {total > covers.length ? (
        <p
          data-enter
          className="text-muted-foreground -mt-4 text-center text-xs tabular-nums"
        >
          …and {fmt(total - covers.length)} more
        </p>
      ) : null}
    </Frame>
  );
}

function Outro({ model, reduced, onReplay }: SlideProps) {
  const time = durationParts(model.activeMs);
  const top = model.top[0];
  const strip = fanCovers(model)
    .concat(model.mosaic.covers.map((c) => c.src))
    .filter((c, i, xs) => xs.indexOf(c) === i)
    .slice(0, 5);
  return (
    <Frame reduced={reduced} className="gap-5">
      <div
        data-enter
        className="text-primary-foreground relative overflow-hidden rounded-2xl bg-[linear-gradient(140deg,var(--primary),color-mix(in_oklab,var(--primary)_70%,#7c2d12))] p-5 shadow-[0_18px_40px_-20px_var(--primary)] sm:p-6"
      >
        <p className="text-xs font-bold tracking-[0.1em] uppercase opacity-80">
          Year in review
        </p>
        <p className="mt-1 text-2xl font-semibold tracking-tight">
          Your {model.year} in comics
        </p>
        <dl className="mt-4 grid grid-cols-2 gap-x-4 gap-y-3 sm:grid-cols-3">
          <Summary label="Pages" value={fmt(model.pages)} />
          <Summary
            label="Finished"
            value={`${fmt(model.mosaic.total || model.issues)}${model.mosaic.more ? "+" : ""}`}
          />
          <Summary
            label={time.unit[0]!.toUpperCase() + time.unit.slice(1)}
            value={fmt(time.value)}
          />
          <Summary label="Reading days" value={fmt(model.daysActive)} />
          <Summary
            label="Best streak"
            value={model.run ? `${model.run.length}d` : "—"}
          />
          {top ? <Summary label="Top series" value={top.name} small /> : null}
        </dl>
        {strip.length ? (
          <div aria-hidden className="mt-4 flex gap-1.5">
            {strip.map((src) => (
              <div key={src} className="w-12 shrink-0 sm:w-14">
                <Cover src={src} alt="" />
              </div>
            ))}
          </div>
        ) : null}
      </div>
      <div
        data-enter
        className="pointer-events-auto flex flex-wrap items-center justify-center gap-2"
      >
        <Button variant="outline" onClick={onReplay}>
          <RotateCcw aria-hidden />
          Watch again
        </Button>
        <Button asChild>
          <Link href="/log">
            Back to your log
            <ArrowRight aria-hidden />
          </Link>
        </Button>
      </div>
    </Frame>
  );
}

function Summary({
  label,
  value,
  small = false,
}: {
  label: string;
  value: string;
  small?: boolean;
}) {
  return (
    <div className="min-w-0">
      <dt className="text-xs opacity-80">{label}</dt>
      <dd
        className={cn(
          "truncate font-semibold tabular-nums",
          small ? "text-sm leading-7" : "text-xl",
        )}
      >
        {value}
      </dd>
    </div>
  );
}

function Empty({ model, reduced, years, onPickYear }: SlideProps) {
  const others = years.filter((y) => y !== model.year).slice(0, 4);
  return (
    <Frame reduced={reduced} className="items-center text-center">
      <p
        data-enter
        className="text-muted-foreground text-[5.5rem] leading-none font-bold tracking-tighter tabular-nums sm:text-9xl"
      >
        {model.year}
      </p>
      <Headline>
        {model.inProgress
          ? "Nothing read yet this year"
          : "No reading this year"}
      </Headline>
      <p data-enter className="text-muted-foreground -mt-3 max-w-xs text-sm">
        {model.inProgress
          ? "Finish an issue and your year in review starts filling in."
          : `Folio has no reading sessions from ${model.year}.`}
      </p>
      <div
        data-enter
        className="pointer-events-auto flex flex-wrap items-center justify-center gap-2"
      >
        {others.map((y) => (
          <Button key={y} variant="outline" onClick={() => onPickYear(y)}>
            See {y}
          </Button>
        ))}
        <Button variant={others.length ? "ghost" : "default"} asChild>
          <Link href="/log">Back to your log</Link>
        </Button>
      </div>
    </Frame>
  );
}
