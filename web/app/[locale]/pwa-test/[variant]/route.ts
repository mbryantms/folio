/**
 * `/pwa-test/{1..7}` — status-bar experiment for the installed iPad app.
 *
 * A route handler, not a page: it returns raw HTML so each variant carries
 * ONLY its own head tags, without the root layout's manifest, apple-* tags
 * and theme-color, which iPadOS snapshots when the icon is added to the
 * Home Screen. Static files under public/ can't be used instead: the CSP's
 * `script-src 'nonce-…' 'strict-dynamic'` would block their script, so the
 * nonce the Rust origin forwards in the request CSP header is stamped onto
 * it here.
 *
 * Unlinked and noindex. Delete this route once the result is in.
 */
import type { NextRequest } from "next/server";

const VARIANTS: Record<
  string,
  { n: string; label: string; desc: string; head: string }
> = {
  "1": {
    n: "1",
    label: "Test 1",
    desc: "apple.com setup: viewport-fit=cover only, black page",
    head: "",
  },
  "2": {
    n: "2",
    label: "Test 2",
    desc: "Test 1 + theme-color #000",
    head: '<meta name="theme-color" content="#000000" />',
  },
  "3": {
    n: "3",
    label: "Test 3",
    desc: "Folio today: capable + status-bar-style black + theme-color",
    head: [
      '<meta name="apple-mobile-web-app-capable" content="yes" />',
      '<meta name="apple-mobile-web-app-status-bar-style" content="black" />',
      '<meta name="theme-color" content="#0c0e13" />',
    ].join("\n  "),
  },
  "4": {
    n: "4",
    label: "Test 4",
    desc: "Test 3 + manifest, display: standalone (Folio's)",
    head: [
      '<meta name="apple-mobile-web-app-capable" content="yes" />',
      '<meta name="apple-mobile-web-app-status-bar-style" content="black" />',
      '<meta name="theme-color" content="#0c0e13" />',
      '<link rel="manifest" href="/pwa-test/manifest/4" />',
    ].join("\n  "),
  },
  "5": {
    n: "5",
    label: "Test 5",
    desc: "Test 3 + color-scheme: dark",
    head: [
      '<meta name="apple-mobile-web-app-capable" content="yes" />',
      '<meta name="apple-mobile-web-app-status-bar-style" content="black" />',
      '<meta name="theme-color" content="#0c0e13" />',
      '<meta name="color-scheme" content="dark" />',
    ].join("\n  "),
  },
  "6": {
    n: "6",
    label: "Test 6",
    desc: "Test 3 + manifest, display: minimal-ui",
    head: [
      '<meta name="apple-mobile-web-app-capable" content="yes" />',
      '<meta name="apple-mobile-web-app-status-bar-style" content="black" />',
      '<meta name="theme-color" content="#0c0e13" />',
      '<link rel="manifest" href="/pwa-test/manifest/6" />',
    ].join("\n  "),
  },
  "7": {
    n: "7",
    label: "Test 7",
    desc: "Test 3 + manifest, display: fullscreen",
    head: [
      '<meta name="apple-mobile-web-app-capable" content="yes" />',
      '<meta name="apple-mobile-web-app-status-bar-style" content="black" />',
      '<meta name="theme-color" content="#0c0e13" />',
      '<link rel="manifest" href="/pwa-test/manifest/7" />',
    ].join("\n  "),
  },
};

export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ variant: string }> },
) {
  const { variant } = await params;
  const v = VARIANTS[variant];
  if (!v) return new Response("Not found", { status: 404 });
  const csp = request.headers.get("content-security-policy") ?? "";
  const nonce = /'nonce-([^']+)'/.exec(csp)?.[1] ?? "";
  const html = `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover" />
  ${v.head}
  <meta name="robots" content="noindex" />
  <title>${v.label}</title>
  <style>
    html, body { margin: 0; background: #000; color: #fff;
      font: 600 17px/1.3 -apple-system, system-ui, sans-serif; }
    header { position: sticky; top: 0; z-index: 10; height: 56px;
      background: #374151; display: flex; align-items: center;
      gap: 16px; padding: 0 20px; }
    header .search { flex: 0 1 380px; height: 32px; border-radius: 8px;
      border: 1px solid #6b7280; }
    .row { height: 120px; display: flex; align-items: center;
      justify-content: space-between; padding: 0 24px; font-size: 28px;
      font-weight: 800; text-shadow: 0 1px 3px rgba(0,0,0,.6); }
    .card { margin: 16px; padding: 16px; border-radius: 12px;
      background: #111; font: 13px/1.5 ui-monospace, monospace; }
    .card button { margin-top: 8px; font: inherit; padding: 6px 12px; }
  </style>
</head>
<body>
  <header>☰ Folio · ${v.label}<span class="search"></span></header>
  <div class="card">
    <div><b>${v.label}</b> — ${v.desc}</div>
    <div>Add this page to the Home Screen, open it from there, screenshot at
      the top, then scroll and screenshot again.</div>
    <pre id="diag"></pre>
    <button id="fs">Try fullscreen</button>
    <div id="fsresult"></div>
  </div>
  <div id="rows"></div>
  <script nonce="${nonce}">
    var rows = document.getElementById("rows");
    for (var i = 0; i < 40; i++) {
      var d = document.createElement("div");
      d.className = "row";
      d.style.background = "linear-gradient(90deg, hsl(" + (i * 37) % 360 +
        " 85% 55%), hsl(" + (i * 37 + 140) % 360 + " 85% 45%))";
      d.innerHTML = "<span>${v.n} · row " + (i + 1) + "</span><span>${v.n}</span>";
      rows.appendChild(d);
    }
    var de = document.documentElement;
    function diag() {
      var p = document.createElement("div");
      p.style.cssText = "position:fixed;visibility:hidden;padding-top:env(safe-area-inset-top,0px)";
      document.body.appendChild(p);
      var env = getComputedStyle(p).paddingTop; p.remove();
      document.getElementById("diag").textContent = [
        "standalone: media=" + matchMedia("(display-mode: standalone)").matches +
          " nav=" + navigator.standalone,
        "inner: " + innerWidth + "×" + innerHeight +
          "  screen: " + screen.width + "×" + screen.height,
        "env(safe-area-inset-top): " + env + "  scrollY: " + Math.round(scrollY),
        "requestFullscreen: " + typeof de.requestFullscreen +
          "  webkitRequestFullscreen: " + typeof de.webkitRequestFullscreen,
        "fullscreenEnabled: " + document.fullscreenEnabled +
          "  webkitFullscreenEnabled: " + document.webkitFullscreenEnabled,
        "fullscreenElement: " + !!(document.fullscreenElement || document.webkitFullscreenElement),
      ].join("\\n");
    }
    diag(); setInterval(diag, 1000);
    document.getElementById("fs").onclick = function () {
      var r = de.requestFullscreen || de.webkitRequestFullscreen;
      var out = document.getElementById("fsresult");
      if (!r) { out.textContent = "No fullscreen API on <html>"; return; }
      try { var x = r.call(de); out.textContent = "Requested"; if (x && x.catch) x.catch(function (e) { out.textContent = "Rejected: " + e; }); }
      catch (e) { out.textContent = "Threw: " + e; }
    };
  </script>
</body>
</html>
`;
  return new Response(html, {
    headers: {
      "content-type": "text/html; charset=utf-8",
      "cache-control": "no-store",
    },
  });
}
