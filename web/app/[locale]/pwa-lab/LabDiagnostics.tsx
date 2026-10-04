"use client";

import { useEffect, useState } from "react";

/** One-glance geometry readout for the status-bar experiments. */
export function LabDiagnostics({ compact = false }: { compact?: boolean }) {
  const [rows, setRows] = useState<[string, string][]>([]);

  useEffect(() => {
    const probe = document.createElement("div");
    probe.style.cssText =
      "position:fixed;top:0;left:0;visibility:hidden;padding-top:env(safe-area-inset-top,0px)";
    document.body.appendChild(probe);
    const read = () => {
      const vv = window.visualViewport;
      const nav = navigator as Navigator & { standalone?: boolean };
      setRows([
        [
          "standalone",
          `media=${window.matchMedia("(display-mode: standalone)").matches} nav=${nav.standalone ?? "n/a"}`,
        ],
        ["inner", `${window.innerWidth}×${window.innerHeight}`],
        ["screen", `${window.screen.width}×${window.screen.height}`],
        [
          "visualViewport",
          vv
            ? `${Math.round(vv.width)}×${Math.round(vv.height)} top=${Math.round(vv.offsetTop)}`
            : "n/a",
        ],
        ["env(top)", getComputedStyle(probe).paddingTop],
        [
          "--safe-top",
          getComputedStyle(document.documentElement)
            .getPropertyValue("--safe-top")
            .trim() || "unset",
        ],
        ["scrollY", String(Math.round(window.scrollY))],
        ["dpr", String(window.devicePixelRatio)],
      ]);
    };
    read();
    const id = window.setInterval(read, 1000);
    return () => {
      window.clearInterval(id);
      probe.remove();
    };
  }, []);

  if (compact) {
    return (
      <p className="font-mono text-[11px] leading-snug">
        {rows.map(([k, v]) => `${k}: ${v}`).join(" · ")}
      </p>
    );
  }
  return (
    <dl className="border-border grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 rounded-lg border p-4 font-mono text-xs">
      {rows.map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="text-muted-foreground">{k}</dt>
          <dd>{v}</dd>
        </div>
      ))}
    </dl>
  );
}
