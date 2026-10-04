"use client";

import Link from "next/link";
import { useEffect, useRef, useState, type CSSProperties } from "react";

import type { LabVariant } from "@/lib/pwa-lab";

import { LabDiagnostics } from "../LabDiagnostics";

const ROWS = 40;
const BAR = 56;
/** How far each variant starts scrolled, so colour is already under the
 *  top edge when the page settles. */
const PRESCROLL = 400;

/** Saturated, high-contrast rows: any blur or tint over them is obvious. */
function Rows({ letter }: { letter: string }) {
  return (
    <>
      {Array.from({ length: ROWS }, (_, i) => (
        <div
          key={i}
          style={{
            height: 120,
            background: `linear-gradient(90deg, hsl(${(i * 37) % 360} 85% 55%), hsl(${(i * 37 + 140) % 360} 85% 45%))`,
            color: "#fff",
            fontWeight: 800,
            fontSize: 28,
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "0 24px",
            textShadow: "0 1px 3px rgba(0,0,0,.6)",
          }}
        >
          <span>
            {letter} · row {i + 1}
          </span>
          <span>{letter}</span>
        </div>
      ))}
    </>
  );
}

/** Bottom-docked readout + back link; kept off the top edge on purpose. */
function Footer({ variant }: { variant: LabVariant }) {
  return (
    <div
      style={{
        position: "fixed",
        left: 8,
        right: 8,
        bottom: "calc(8px + env(safe-area-inset-bottom, 0px))",
        zIndex: 50,
        background: "rgba(0,0,0,.85)",
        color: "#fff",
        borderRadius: 10,
        padding: "8px 12px",
      }}
    >
      <div
        style={{ display: "flex", justifyContent: "space-between", gap: 12 }}
      >
        <strong>
          {variant.letter}. {variant.title}
        </strong>
        <Link href="/pwa-lab" style={{ textDecoration: "underline" }}>
          All variants
        </Link>
      </div>
      <LabDiagnostics compact />
    </div>
  );
}

/** Lock document scroll for the inner-scroller variants; restore on leave. */
function useLockedDocument(locked: boolean) {
  useEffect(() => {
    if (!locked) return;
    const html = document.documentElement;
    const body = document.body;
    const prev = [html.style.cssText, body.style.cssText];
    html.style.cssText +=
      ";overflow:hidden;height:100%;overscroll-behavior:none";
    body.style.cssText +=
      ";overflow:hidden;height:100%;margin:0;overscroll-behavior:none;background:#000";
    return () => {
      html.style.cssText = prev[0] ?? "";
      body.style.cssText = prev[1] ?? "";
    };
  }, [locked]);
}

/** Black document background for every variant, so nothing but the rows
 *  carries colour. */
function useBlackBody() {
  useEffect(() => {
    const prev = document.body.style.background;
    document.body.style.background = "#000";
    return () => {
      document.body.style.background = prev;
    };
  }, []);
}

/** Fixed full-width solid band at the top edge. */
function Band({ height, color }: { height: number; color: string }) {
  return (
    <div
      style={{
        position: "fixed",
        top: 0,
        left: 0,
        right: 0,
        height,
        background: color,
        zIndex: 30,
      }}
    />
  );
}

/** The scrolling region: everything below `top`, document stays still. */
function InnerScroller({
  top,
  scrollerRef,
  children,
  onClick,
}: {
  top: number;
  scrollerRef: React.RefObject<HTMLDivElement | null>;
  children: React.ReactNode;
  onClick?: () => void;
}) {
  return (
    <div
      ref={scrollerRef}
      onClick={onClick}
      style={{
        position: "fixed",
        top,
        left: 0,
        right: 0,
        bottom: 0,
        overflowY: "auto",
        overscrollBehavior: "contain",
      }}
    >
      {children}
    </div>
  );
}

/** Reader mock: a thin band always; tapping toggles a chrome bar that
 *  overlays the top of the scroller (like ReaderChrome). */
function ReaderMock({
  variant,
  scrollerRef,
}: {
  variant: LabVariant;
  scrollerRef: React.RefObject<HTMLDivElement | null>;
}) {
  const [chrome, setChrome] = useState(true);
  return (
    <>
      <Band height={12} color="#000" />
      {chrome ? (
        <div
          style={{
            position: "fixed",
            top: 0,
            left: 0,
            right: 0,
            height: 56,
            background: "#0a0a0a",
            borderBottom: "1px solid #262626",
            color: "#e5e5e5",
            zIndex: 40,
            display: "flex",
            alignItems: "center",
            padding: "0 16px",
            fontSize: 15,
          }}
        >
          ← Pages 6–7 / 31 · tap content to hide chrome
        </div>
      ) : null}
      <InnerScroller
        top={12}
        scrollerRef={scrollerRef}
        onClick={() => setChrome((c) => !c)}
      >
        <Rows letter="I" />
      </InnerScroller>
      <Footer variant={variant} />
    </>
  );
}

export function LabVariantView({ variant }: { variant: LabVariant }) {
  // Every variant from C on keeps the document still and scrolls inside.
  const inner = !["A", "B", "E"].includes(variant.letter);
  const scroller = useRef<HTMLDivElement>(null);
  useBlackBody();
  useLockedDocument(inner);

  useEffect(() => {
    // E starts at rest (its whole point is the at-rest vs scrolled compare).
    if (variant.id.startsWith("e-")) return;
    const t = window.setTimeout(() => {
      if (inner) scroller.current?.scrollTo(0, PRESCROLL);
      else window.scrollTo(0, PRESCROLL);
    }, 150);
    return () => window.clearTimeout(t);
  }, [inner, variant.id]);

  const solidBar: CSSProperties = {
    height: BAR,
    background: "#000",
    width: "100%",
  };

  switch (variant.letter) {
    case "B":
      return (
        <>
          <div
            style={{
              ...solidBar,
              position: "fixed",
              top: 0,
              left: 0,
              zIndex: 40,
            }}
          />
          <Rows letter="B" />
          <Footer variant={variant} />
        </>
      );
    case "C":
      return (
        <>
          <div
            ref={scroller}
            style={{
              position: "fixed",
              inset: 0,
              overflowY: "auto",
              overscrollBehavior: "contain",
            }}
          >
            <Rows letter="C" />
          </div>
          <Footer variant={variant} />
        </>
      );
    case "D":
      return (
        <>
          <div style={{ ...solidBar, position: "fixed", top: 0, left: 0 }} />
          <div
            ref={scroller}
            style={{
              position: "fixed",
              top: BAR,
              left: 0,
              right: 0,
              bottom: 0,
              overflowY: "auto",
              overscrollBehavior: "contain",
            }}
          >
            <Rows letter="D" />
          </div>
          <Footer variant={variant} />
        </>
      );
    case "F":
    case "G": {
      const h = variant.letter === "F" ? 12 : 4;
      return (
        <>
          <Band height={h} color="#000" />
          <InnerScroller top={h} scrollerRef={scroller}>
            <Rows letter={variant.letter} />
          </InnerScroller>
          <Footer variant={variant} />
        </>
      );
    }
    case "H":
      return (
        <>
          <div
            style={{
              position: "fixed",
              top: 0,
              left: 0,
              right: 0,
              height: BAR,
              zIndex: 30,
              background: "hsl(var(--background))",
              borderBottom: "1px solid hsl(var(--border))",
              color: "hsl(var(--foreground))",
              display: "flex",
              alignItems: "center",
              gap: 16,
              padding: "0 24px",
              fontSize: 17,
              fontWeight: 600,
            }}
          >
            ☰ Folio
            <span
              style={{
                flex: "0 1 420px",
                height: 34,
                borderRadius: 8,
                border: "1px solid hsl(var(--border))",
              }}
            />
          </div>
          <InnerScroller top={BAR} scrollerRef={scroller}>
            <Rows letter="H" />
          </InnerScroller>
          <Footer variant={variant} />
        </>
      );
    case "I":
      return <ReaderMock variant={variant} scrollerRef={scroller} />;
    case "E":
      return (
        <>
          <div style={{ height: 240, background: "#000" }} />
          <Rows letter="E" />
          <Footer variant={variant} />
        </>
      );
    default:
      return (
        <>
          <Rows letter="A" />
          <Footer variant={variant} />
        </>
      );
  }
}
