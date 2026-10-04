import type { Metadata } from "next";
import { notFound } from "next/navigation";

import { labVariant } from "@/lib/pwa-lab";

import { LabVariantView } from "./LabVariantView";

export const metadata: Metadata = {
  title: "PWA lab · Folio",
  robots: { index: false, follow: false },
};

export default async function PwaLabVariant({
  params,
}: {
  params: Promise<{ variant: string }>;
}) {
  const { variant } = await params;
  const v = labVariant(variant);
  if (!v) notFound();
  return <LabVariantView variant={v} />;
}
