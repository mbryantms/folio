/**
 * Pure helpers for /me/sessions UI. Kept out of SessionsCard.tsx so vitest
 * (node-env) can exercise them without spinning up jsdom.
 */

/**
 * Best-effort UA → "Browser on OS" shortener. We deliberately don't pull in
 * `ua-parser-js`: the strings we get from mainstream browsers all match a
 * handful of regexes, and a 200-byte lookup beats shipping a 30 kB dep.
 */
export function prettyUserAgent(ua: string | null | undefined): {
  device: string;
  raw: string;
} {
  if (!ua || ua.trim().length === 0) {
    return { device: "Unknown device", raw: "" };
  }
  const raw = ua;
  const lower = ua.toLowerCase();

  let os = "Unknown OS";
  // iOS check must precede macOS — Safari on iOS UAs contain "like Mac OS X"
  // and would otherwise mis-classify as macOS.
  if (
    lower.includes("iphone") ||
    lower.includes("ipad") ||
    lower.includes("ipod")
  )
    os = "iOS";
  else if (lower.includes("android")) os = "Android";
  else if (lower.includes("windows")) os = "Windows";
  else if (lower.includes("mac os") || lower.includes("macintosh"))
    os = "macOS";
  else if (lower.includes("cros")) os = "ChromeOS";
  else if (lower.includes("linux")) os = "Linux";

  let browser = "Browser";
  // Order matters: Edge / Opera UAs include Chrome/, so they must be
  // matched before Chrome. Same for Safari (also includes Chrome/) — we
  // check Chrome first via the !chromium guard but explicitly exclude
  // Chrome-containing UAs from Safari.
  if (lower.includes("edg/")) browser = "Edge";
  else if (lower.includes("opr/") || lower.includes("opera/"))
    browser = "Opera";
  else if (lower.includes("firefox/")) browser = "Firefox";
  else if (lower.includes("chrome/") && !lower.includes("chromium"))
    browser = "Chrome";
  else if (lower.includes("safari/") && !lower.includes("chrome"))
    browser = "Safari";
  else if (lower.includes("curl/")) browser = "curl";
  else if (lower.includes("wget/")) browser = "wget";

  return { device: `${browser} on ${os}`, raw };
}

/**
 * Relative-time formatter. Returns "just now", "5 mins ago", "3 hours ago",
 * "2 days ago", or a localized date for anything past 30 days. `now` is
 * injectable for deterministic tests.
 */
export function timeAgo(iso: string, now: number = Date.now()): string {
  const then = new Date(iso).getTime();
  if (!Number.isFinite(then)) return iso;
  const diff = Math.max(0, now - then);
  const sec = Math.floor(diff / 1000);
  if (sec < 60) return "just now";
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min} min${min === 1 ? "" : "s"} ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr} hour${hr === 1 ? "" : "s"} ago`;
  const day = Math.floor(hr / 24);
  if (day < 30) return `${day} day${day === 1 ? "" : "s"} ago`;
  return new Date(iso).toLocaleDateString();
}
