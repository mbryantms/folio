/** Verify the shipped public origin, not Next's private upstream. Brand
 * blockers are reported explicitly until the existing asset work is complete.
 * Set PWA_REQUIRE_BRAND=1 to make those missing assets fatal. */
const origin = process.env.PLAYWRIGHT_BASE_URL ?? process.argv[2];
if (!origin)
  throw new Error("Set PLAYWRIGHT_BASE_URL or pass the public origin");
const failures = [],
  blocked = [];
async function get(path, type, brand = false) {
  const response = await fetch(new URL(path, origin));
  if (!response.ok || !response.headers.get("content-type")?.includes(type)) {
    (brand && !process.env.PWA_REQUIRE_BRAND ? blocked : failures).push(
      `${path}: ${response.status} ${response.headers.get("content-type")}`,
    );
    return null;
  }
  return response;
}
const manifestResponse = await get("/manifest.webmanifest", "json");
if (manifestResponse) {
  const manifest = await manifestResponse.json();
  if (manifest.id !== "/" || manifest.scope !== "/")
    failures.push("Unexpected manifest identity/scope");
  for (const icon of manifest.icons ?? []) {
    const response = await get(icon.src, icon.type, true);
    if (!response || icon.type !== "image/png") continue;
    const bytes = new DataView(await response.arrayBuffer());
    const expected = icon.sizes.split("x").map(Number);
    if (
      bytes.byteLength < 24 ||
      bytes.getUint32(16) !== expected[0] ||
      bytes.getUint32(20) !== expected[1]
    )
      failures.push(`${icon.src}: invalid PNG dimensions`);
  }
}
const html = await (await get("/sign-in", "text/html"))?.text();
for (const tag of html?.match(/<link\b[^>]*>/g) ?? []) {
  if (!/rel="apple-touch-(?:startup-image|icon)"/.test(tag)) continue;
  const path = tag.match(/href="([^"]+)"/)?.[1];
  if (path) await get(path, "image/png", true);
}
const worker = await get("/sw.js", "javascript");
if (
  worker &&
  !/no-cache|no-store|max-age=0/.test(worker.headers.get("cache-control") ?? "")
)
  failures.push("Worker must revalidate");
await get("/offline.html", "text/html");
if (blocked.length)
  console.warn(
    "BLOCKED: final brand assets are not available:\n" + blocked.join("\n"),
  );
if (failures.length) throw new Error(failures.join("\n"));
console.warn(
  "PWA origin checks passed" +
    (blocked.length ? `; ${blocked.length} brand assets blocked` : ""),
);
