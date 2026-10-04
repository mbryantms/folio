"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, useState } from "react";

import type { LabVariant } from "@/lib/pwa-lab";

/**
 * Latch probe for the iPadOS status-bar strip (see lib/pwa-lab.ts).
 *
 * The strip is either painted in the top-edge bar's solid colour (clean)
 * or as a blur of the scrolled content (faded), and once an installed app
 * flips to the blur it stays there until force-quit. This page:
 *
 *  - has a sticky grey header, so the strip's state is visible at a glance;
 *  - starts a document-wide logger (survives in-app navigation: Folio is
 *    one document) that records every <meta> change in <head> and every
 *    change of the element iOS samples at the top edge, with the route;
 *  - has one button per candidate trigger, each applied for two seconds,
 *    plus a real reload.
 *
 * Procedure: force-quit Folio, open this page, confirm the strip is grey,
 * press ONE button, look at the strip, note it with "clean" / "faded".
 * Repeat from a force-quit for each button. Finally: note, go to the real
 * reader, come back here, read the log.
 */

type LogEntry = { t: number; path: string; msg: string };

declare global {
  interface Window {
    __folioProbeLog?: LogEntry[];
    __folioProbeStarted?: boolean;
  }
}

function tag(el: Element): string {
  const cls = (el.getAttribute("class") ?? "")
    .split(/\s+/)
    .slice(0, 3)
    .join(".");
  return `${el.tagName.toLowerCase()}${el.id ? `#${el.id}` : ""}${cls ? `.${cls}` : ""}`;
}

/** What iOS sees at (centre, 4px): the nearest fixed/sticky ancestor of
 *  the hit element, ignoring pointer-events like WebKit does. */
function sampleTopEdge(): string {
  const st = document.createElement("style");
  st.textContent = "*{pointer-events:auto!important}";
  document.head.appendChild(st);
  try {
    const hit = document.elementFromPoint(window.innerWidth / 2, 4);
    if (!hit) return "nothing";
    let el: Element | null = hit;
    while (el && el !== document.documentElement) {
      const cs = getComputedStyle(el);
      if (cs.position === "fixed" || cs.position === "sticky") {
        const r = el.getBoundingClientRect();
        return `${cs.position} ${tag(el)} ${Math.round(r.width)}x${Math.round(r.height)} bg=${cs.backgroundColor} bdf=${cs.backdropFilter} op=${cs.opacity}`;
      }
      el = el.parentElement;
    }
    return `no fixed/sticky (hit ${tag(hit)})`;
  } finally {
    st.remove();
  }
}

function startLogger() {
  if (window.__folioProbeStarted) return;
  window.__folioProbeStarted = true;
  const log = (window.__folioProbeLog ??= []);
  const push = (msg: string) => {
    log.push({ t: Date.now(), path: location.pathname, msg });
    if (log.length > 500) log.splice(0, log.length - 500);
  };
  push("logger started");
  const meta = (n: Node) =>
    n instanceof HTMLMetaElement
      ? `meta[${n.getAttribute("name") ?? n.getAttribute("property") ?? "?"}${n.media ? ` media=${n.media}` : ""}]=${n.content}`
      : null;
  const obs = new MutationObserver((muts) => {
    for (const m of muts) {
      if (m.type === "attributes") {
        const d = meta(m.target);
        if (d) push(`head attr ${m.attributeName}: ${m.oldValue} -> ${d}`);
        continue;
      }
      m.addedNodes.forEach((n) => {
        const d = meta(n);
        if (d) push(`head + ${d}`);
      });
      m.removedNodes.forEach((n) => {
        const d = meta(n);
        if (d) push(`head - ${d}`);
      });
    }
  });
  obs.observe(document.head, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeOldValue: true,
    attributeFilter: ["content", "name", "media"],
  });
  let lastEdge = "";
  let lastPath = location.pathname;
  window.setInterval(() => {
    if (location.pathname !== lastPath) {
      lastPath = location.pathname;
      push("route");
    }
    const e = sampleTopEdge();
    if (e !== lastEdge) {
      lastEdge = e;
      push(`edge: ${e}`);
    }
  }, 400);
}

const HOLD_MS = 2000;

function forMs(apply: () => () => void) {
  const undo = apply();
  window.setTimeout(undo, HOLD_MS);
}

const TRIGGERS: { label: string; run: () => void }[] = [
  {
    label: "theme-color → red → back",
    run: () =>
      forMs(() => {
        const metas = [
          ...document.querySelectorAll<HTMLMetaElement>(
            'meta[name="theme-color"]',
          ),
        ];
        const prev = metas.map((m) => m.content);
        metas.forEach((m) => (m.content = "#ff0000"));
        return () => metas.forEach((m, i) => (m.content = prev[i] ?? ""));
      }),
  },
  {
    // Exactly what Next's metadata tree did on EVERY client navigation
    // before the root layout took these tags over: remove them all, then
    // re-insert identical copies in the same commit.
    label: "churn: remove + re-add all app metas (old Next behaviour)",
    run: () =>
      forMs(() => {
        const metas = [
          ...document.querySelectorAll<HTMLMetaElement>(
            'meta[name="theme-color"], meta[name="color-scheme"], meta[name^="apple-mobile-web-app"], meta[name="mobile-web-app-capable"]',
          ),
        ];
        const clones = metas.map((m) => m.cloneNode(true) as HTMLMetaElement);
        metas.forEach((m) => m.remove());
        clones.forEach((c) => document.head.appendChild(c));
        return () => undefined;
      }),
  },
  {
    label: "churn, slow: remove all app metas, re-add after 300ms",
    run: () =>
      forMs(() => {
        const metas = [
          ...document.querySelectorAll<HTMLMetaElement>(
            'meta[name="theme-color"], meta[name="color-scheme"], meta[name^="apple-mobile-web-app"], meta[name="mobile-web-app-capable"]',
          ),
        ];
        const clones = metas.map((m) => m.cloneNode(true) as HTMLMetaElement);
        metas.forEach((m) => m.remove());
        window.setTimeout(
          () => clones.forEach((c) => document.head.appendChild(c)),
          300,
        );
        return () => undefined;
      }),
  },
  {
    // What Next still does on every client navigation after #1016: the
    // charset, viewport, description and robots metas are removed and
    // re-inserted with identical values, in one commit.
    label: "churn: remove + re-add the viewport meta (identical)",
    run: () =>
      forMs(() => {
        const m = document.querySelector<HTMLMetaElement>(
          'meta[name="viewport"]',
        );
        if (!m) return () => undefined;
        const clone = m.cloneNode(true) as HTMLMetaElement;
        const next = m.nextSibling;
        m.remove();
        document.head.insertBefore(clone, next);
        return () => undefined;
      }),
  },
  {
    label: "churn, slow: remove the viewport meta, re-add after 300ms",
    run: () =>
      forMs(() => {
        const m = document.querySelector<HTMLMetaElement>(
          'meta[name="viewport"]',
        );
        if (!m) return () => undefined;
        const clone = m.cloneNode(true) as HTMLMetaElement;
        m.remove();
        window.setTimeout(() => document.head.appendChild(clone), 300);
        return () => undefined;
      }),
  },
  {
    label: "color-scheme → dark → back",
    run: () =>
      forMs(() => {
        const m = document.querySelector<HTMLMetaElement>(
          'meta[name="color-scheme"]',
        );
        if (!m) return () => undefined;
        const prev = m.content;
        m.content = "dark";
        return () => (m.content = prev);
      }),
  },
  {
    label: "viewport-sized transparent fixed overlay",
    run: () =>
      forMs(() => {
        const d = document.createElement("div");
        d.style.cssText =
          "position:fixed;inset:0;z-index:70;background:transparent";
        document.body.appendChild(d);
        return () => d.remove();
      }),
  },
  {
    label: "12px fixed black bar at top",
    run: () =>
      forMs(() => {
        const d = document.createElement("div");
        d.style.cssText =
          "position:fixed;top:0;left:0;right:0;height:12px;z-index:70;background:#000";
        document.body.appendChild(d);
        return () => d.remove();
      }),
  },
  {
    label: "56px fixed black bar at top",
    run: () =>
      forMs(() => {
        const d = document.createElement("div");
        d.style.cssText =
          "position:fixed;top:0;left:0;right:0;height:56px;z-index:70;background:#000";
        document.body.appendChild(d);
        return () => d.remove();
      }),
  },
  {
    label: "hide the sticky header (no top bar at all)",
    run: () =>
      forMs(() => {
        const h = document.getElementById("probe-header");
        if (!h) return () => undefined;
        h.style.display = "none";
        return () => (h.style.display = "");
      }),
  },
  {
    label: "lock document scroll (overflow hidden on html)",
    run: () =>
      forMs(() => {
        const prev = document.documentElement.style.overflow;
        document.documentElement.style.overflow = "hidden";
        return () => (document.documentElement.style.overflow = prev);
      }),
  },
];

function addNote(what: string) {
  window.__folioProbeLog?.push({
    t: Date.now(),
    path: location.pathname,
    msg: `NOTE: ${what}`,
  });
}

function snapshot(): string {
  return (window.__folioProbeLog ?? [])
    .map(
      (e) => `${new Date(e.t).toISOString().slice(11, 23)} ${e.path} ${e.msg}`,
    )
    .join("\n");
}

export function LatchProbe({ variant }: { variant: LabVariant }) {
  const [text, setText] = useState("");
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
  const router = useRouter();
  // A real Next client navigation that lands on THIS page again (new query
  // string): the head churn and route transition happen while the sticky
  // header never leaves the screen — head churn isolated from "no top bar".
  const softNavigate = () => {
    note("trigger: soft navigation to this page");
    router.push(`/pwa-lab/p-latch-probe?n=${Date.now()}`);
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
      <div style={{ padding: 16, display: "flex", flexWrap: "wrap", gap: 8 }}>
        {TRIGGERS.map((t) => (
          <button
            key={t.label}
            type="button"
            onClick={() => {
              note(`trigger: ${t.label}`);
              t.run();
            }}
            style={{
              padding: "10px 14px",
              borderRadius: 8,
              background: "#1f2937",
              color: "#fff",
            }}
          >
            {t.label}
          </button>
        ))}
        <button
          type="button"
          onClick={softNavigate}
          style={{
            padding: "10px 14px",
            borderRadius: 8,
            background: "#4c1d95",
            color: "#fff",
          }}
        >
          soft-navigate to this page (Next router)
        </button>
        <button
          type="button"
          onClick={() => window.location.reload()}
          style={{
            padding: "10px 14px",
            borderRadius: 8,
            background: "#7c2d12",
            color: "#fff",
          }}
        >
          Reload (real)
        </button>
        <button
          type="button"
          onClick={() => note("strip looks CLEAN")}
          style={{
            padding: "10px 14px",
            borderRadius: 8,
            background: "#14532d",
            color: "#fff",
          }}
        >
          note: clean
        </button>
        <button
          type="button"
          onClick={() => note("strip looks FADED")}
          style={{
            padding: "10px 14px",
            borderRadius: 8,
            background: "#7f1d1d",
            color: "#fff",
          }}
        >
          note: faded
        </button>
        <button
          type="button"
          onClick={() => void navigator.clipboard?.writeText(text)}
          style={{
            padding: "10px 14px",
            borderRadius: 8,
            background: "#1e3a8a",
            color: "#fff",
          }}
        >
          Copy log
        </button>
        <button
          type="button"
          onClick={() => {
            if (window.__folioProbeLog) window.__folioProbeLog.length = 0;
            setText("");
          }}
          style={{
            padding: "10px 14px",
            borderRadius: 8,
            background: "#374151",
            color: "#fff",
          }}
        >
          Clear log
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
          maxHeight: "40vh",
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
          P · row {i + 1}
        </div>
      ))}
    </>
  );
}
