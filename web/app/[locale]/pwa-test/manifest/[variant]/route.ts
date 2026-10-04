/**
 * Web app manifests for `/pwa-test/{4,6,7}` — one per variant, each with
 * its own `id`, `start_url` and `scope`, so every test installs as a
 * separate Home Screen app that opens on its own page. Only `display`
 * differs. Delete with the rest of `pwa-test/`.
 */
const DISPLAY: Record<string, "standalone" | "minimal-ui" | "fullscreen"> = {
  "4": "standalone",
  "6": "minimal-ui",
  "7": "fullscreen",
};

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ variant: string }> },
) {
  const { variant } = await params;
  const display = DISPLAY[variant];
  if (!display) return new Response("Not found", { status: 404 });
  const path = `/pwa-test/${variant}`;
  return Response.json(
    {
      id: path,
      name: `Test ${variant}`,
      short_name: `Test ${variant}`,
      start_url: path,
      scope: path,
      display,
      background_color: "#000000",
      theme_color: "#0c0e13",
      icons: [
        { src: "/icons/icon-192.png", sizes: "192x192", type: "image/png" },
        { src: "/icons/icon-512.png", sizes: "512x512", type: "image/png" },
      ],
    },
    {
      headers: {
        "content-type": "application/manifest+json",
        "cache-control": "no-store",
      },
    },
  );
}
