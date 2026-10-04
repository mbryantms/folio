import type { Metadata } from "next";
import Link from "next/link";

import { LAB_VARIANTS } from "@/lib/pwa-lab";

import { LabDiagnostics } from "./LabDiagnostics";

export const metadata: Metadata = {
  title: "PWA lab · Folio",
  robots: { index: false, follow: false },
};

/**
 * `/pwa-lab` — index of the status-bar fade experiments (see
 * `lib/pwa-lab.ts`). Open from the installed app; each link stays in-app.
 */
export default function PwaLabIndex() {
  return (
    <main className="mx-auto max-w-2xl px-4 pt-24 pb-16">
      <h1 className="text-2xl font-semibold">PWA status-bar lab</h1>
      <p className="text-muted-foreground mt-2 text-sm">
        Open each variant from the installed app, wait a second, screenshot,
        then scroll a little and screenshot again. The letter is repeated in
        every row so it shows in any crop.
      </p>
      <ul className="mt-6 space-y-3">
        {LAB_VARIANTS.map((v) => (
          <li key={v.id}>
            <Link
              href={`/pwa-lab/${v.id}`}
              className="border-border hover:bg-muted block rounded-lg border p-4"
            >
              <span className="font-semibold">
                {v.letter}. {v.title}
              </span>
              <span className="text-muted-foreground mt-1 block text-sm">
                {v.question}
              </span>
            </Link>
          </li>
        ))}
      </ul>
      <div className="mt-8">
        <LabDiagnostics />
      </div>
    </main>
  );
}
