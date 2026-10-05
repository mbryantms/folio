"use client";

import Link from "next/link";
import { useEffect, useState } from "react";

import type { LabVariant } from "@/lib/pwa-lab";

import { addNote, snapshot, startLogger } from "./LatchProbe";

/**
 * Parameterised version of the latch probe: instead of one button per
 * guess, a form describes the bar to leave at the top edge, so a whole
 * matrix (fixed/sticky × height × scrollable document × header kept or
 * hidden) runs from one page with no code change. Served from the dev
 * box over the LAN, every change is live on Reload. Delete with the lab.
 */

type Position = "fixed" | "sticky" | "shrink-header";

interface Params {
  position: Position;
  height: number;
  color: string;
  zIndex: number;
  hideHeader: boolean;
  lockScroll: boolean;
  durationMs: number;
}

const DEFAULTS: Params = {
  position: "fixed",
  height: 56,
  color: "#374151",
  zIndex: 1,
  hideHeader: true,
  lockScroll: false,
  durationMs: 2000,
};

function apply(p: Params): () => void {
  const header = document.getElementById("probe-header");
  const undo: (() => void)[] = [];
  if (p.lockScroll) {
    const prev = document.documentElement.style.overflow;
    document.documentElement.style.overflow = "hidden";
    undo.push(() => (document.documentElement.style.overflow = prev));
  }
  if (p.position === "shrink-header") {
    if (header) {
      const prev = header.style.cssText;
      header.style.height = `${p.height}px`;
      header.style.overflow = "hidden";
      header.style.color = "transparent";
      header.style.background = p.color;
      undo.push(() => (header.style.cssText = prev));
    }
  } else {
    const d = document.createElement("div");
    d.setAttribute("data-experiment-bar", "");
    d.style.cssText = `position:${p.position};top:0;left:0;right:0;height:${p.height}px;z-index:${p.zIndex};background:${p.color}`;
    // Sticky must be in normal flow at the top of the scroller: first
    // child of body. Fixed can go anywhere; body start keeps it simple.
    document.body.prepend(d);
    undo.push(() => d.remove());
    if (p.hideHeader && header) {
      const prev = header.style.display;
      header.style.display = "none";
      undo.push(() => (header.style.display = prev));
    }
  }
  return () => undo.reverse().forEach((f) => f());
}

export function ExperimentConsole({ variant }: { variant: LabVariant }) {
  const [p, setP] = useState<Params>(DEFAULTS);
  const [text, setText] = useState("");
  const [active, setActive] = useState<(() => void) | null>(null);

  useEffect(() => {
    startLogger();
    const refresh = () => setText(snapshot());
    refresh();
    const id = window.setInterval(refresh, 1000);
    return () => window.clearInterval(id);
  }, []);

  const note = (what: string) => {
    addNote(what);
    setText(snapshot());
  };
  const run = () => {
    if (active) active();
    note(
      `trigger: ${p.position} ${p.height}px ${p.color} z=${p.zIndex} header=${p.hideHeader ? "hidden" : "kept"} scroll=${p.lockScroll ? "locked" : "free"} for ${p.durationMs || "∞"}ms`,
    );
    const undo = apply(p);
    if (p.durationMs > 0) {
      window.setTimeout(() => {
        undo();
        setActive(null);
      }, p.durationMs);
      setActive(null);
    } else {
      setActive(() => undo);
    }
  };
  const restore = () => {
    if (active) {
      active();
      setActive(null);
      note("restored");
    }
  };
  const set = <K extends keyof Params>(k: K, v: Params[K]) =>
    setP((q) => ({ ...q, [k]: v }));

  const btn: React.CSSProperties = {
    padding: "10px 14px",
    borderRadius: 8,
    background: "#1f2937",
    color: "#fff",
  };
  const field: React.CSSProperties = {
    display: "flex",
    flexDirection: "column",
    gap: 4,
    font: "13px -apple-system, system-ui, sans-serif",
    color: "#ddd",
  };
  const input: React.CSSProperties = {
    padding: "8px 10px",
    borderRadius: 8,
    background: "#111",
    color: "#fff",
    border: "1px solid #444",
    minWidth: 120,
  };

  return (
    <>
      <header
        id="probe-header"
        style={{
          position: "sticky",
          top: 0,
          zIndex: 10,
          height: 56,
          background: "#374151",
          color: "#fff",
          display: "flex",
          alignItems: "center",
          gap: 16,
          padding: "0 20px",
          font: "600 17px/1.3 -apple-system, system-ui, sans-serif",
        }}
      >
        ☰ Folio · {variant.title}
        <Link
          href="/pwa-lab"
          style={{ marginLeft: "auto", textDecoration: "underline" }}
        >
          All variants
        </Link>
      </header>

      <div
        style={{
          padding: 16,
          display: "flex",
          flexWrap: "wrap",
          gap: 12,
          alignItems: "flex-end",
        }}
      >
        <label style={field}>
          bar position
          <select
            style={input}
            value={p.position}
            onChange={(e) => set("position", e.target.value as Position)}
          >
            <option value="fixed">fixed (new bar)</option>
            <option value="sticky">sticky (new bar, in flow)</option>
            <option value="shrink-header">shrink the sticky header</option>
          </select>
        </label>
        <label style={field}>
          height (px)
          <input
            style={input}
            type="number"
            value={p.height}
            onChange={(e) => set("height", Number(e.target.value))}
          />
        </label>
        <label style={field}>
          colour
          <input
            style={input}
            type="text"
            value={p.color}
            onChange={(e) => set("color", e.target.value)}
          />
        </label>
        <label style={field}>
          z-index
          <input
            style={input}
            type="number"
            value={p.zIndex}
            onChange={(e) => set("zIndex", Number(e.target.value))}
          />
        </label>
        <label style={field}>
          duration (ms, 0 = until Restore)
          <input
            style={input}
            type="number"
            value={p.durationMs}
            onChange={(e) => set("durationMs", Number(e.target.value))}
          />
        </label>
        <label style={{ ...field, flexDirection: "row", alignItems: "center" }}>
          <input
            type="checkbox"
            checked={p.hideHeader}
            onChange={(e) => set("hideHeader", e.target.checked)}
          />
          hide the real header
        </label>
        <label style={{ ...field, flexDirection: "row", alignItems: "center" }}>
          <input
            type="checkbox"
            checked={p.lockScroll}
            onChange={(e) => set("lockScroll", e.target.checked)}
          />
          lock document scroll
        </label>
      </div>

      <div
        style={{
          padding: "0 16px 16px",
          display: "flex",
          flexWrap: "wrap",
          gap: 8,
        }}
      >
        <button
          type="button"
          onClick={run}
          style={{ ...btn, background: "#4c1d95" }}
        >
          Apply
        </button>
        <button
          type="button"
          onClick={() => {
            const root = document.documentElement;
            const prev = root.style.getPropertyValue("--top-edge-color");
            root.style.setProperty("--top-edge-color", "#000");
            note("trigger: #top-edge colour -> #000 (same element) for 2000ms");
            window.setTimeout(() => {
              if (prev) root.style.setProperty("--top-edge-color", prev);
              else root.style.removeProperty("--top-edge-color");
            }, 2000);
          }}
          style={{ ...btn, background: "#0f766e" }}
        >
          #top-edge → black (same element)
        </button>
        <button type="button" onClick={restore} style={btn} disabled={!active}>
          Restore
        </button>
        <button
          type="button"
          onClick={() => window.location.reload()}
          style={{ ...btn, background: "#7c2d12" }}
        >
          Reload (real)
        </button>
        <button
          type="button"
          onClick={() => note("strip looks CLEAN")}
          style={{ ...btn, background: "#14532d" }}
        >
          note: clean
        </button>
        <button
          type="button"
          onClick={() => note("strip looks FADED")}
          style={{ ...btn, background: "#7f1d1d" }}
        >
          note: faded
        </button>
        <button
          type="button"
          onClick={() => void navigator.clipboard?.writeText(text)}
          style={{ ...btn, background: "#1e3a8a" }}
        >
          Copy log
        </button>
      </div>

      <pre
        style={{
          margin: 16,
          padding: 12,
          borderRadius: 10,
          background: "#111",
          color: "#ddd",
          font: "12px/1.45 ui-monospace, monospace",
          whiteSpace: "pre-wrap",
          maxHeight: "30vh",
          overflow: "auto",
        }}
      >
        {text || "(log empty)"}
      </pre>

      {Array.from({ length: 30 }, (_, i) => (
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
            padding: "0 24px",
            textShadow: "0 1px 3px rgba(0,0,0,.6)",
          }}
        >
          Q · row {i + 1}
        </div>
      ))}
    </>
  );
}
