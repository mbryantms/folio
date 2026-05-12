import Link from "next/link";
import { Chrome } from "@/components/Chrome";

export default function LocaleNotFound() {
  return (
    <Chrome breadcrumbs={[{ label: "Not found" }]}>
      <div className="mx-auto max-w-md py-16 text-center">
        <p className="text-xs tracking-widest text-neutral-500 uppercase">
          404
        </p>
        <h1 className="mt-2 text-3xl font-semibold tracking-tight">
          Not found.
        </h1>
        <p className="mt-4 text-sm text-neutral-400">
          That page doesn&apos;t exist (or you don&apos;t have access to it).
        </p>
        <Link
          href="/"
          className="mt-8 inline-block rounded-md border border-neutral-700 px-4 py-2 text-sm hover:bg-neutral-900"
        >
          Back to library →
        </Link>
      </div>
    </Chrome>
  );
}
