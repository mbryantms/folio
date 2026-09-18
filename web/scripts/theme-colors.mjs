import { readFileSync, writeFileSync } from "node:fs";
const css = readFileSync(
  new URL("../styles/globals.css", import.meta.url),
  "utf8",
);
const colors = {};
for (const theme of ["dark", "light", "amber"]) {
  const block = css.match(
    new RegExp('\\[data-theme="' + theme + '"\\]\\s*\\{([\\s\\S]*?)\\n\\}'),
  )?.[1];
  const match = block?.match(/--background:\s*([\d.]+) ([\d.]+)% ([\d.]+)%/);
  if (!match) throw new Error(`Missing ${theme} background`);
  const h = Number(match[1]),
    s = Number(match[2]) / 100,
    l = Number(match[3]) / 100;
  const a = s * Math.min(l, 1 - l);
  const channel = (n) => {
    const k = (n + h / 30) % 12;
    return Math.round(255 * (l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1))))
      .toString(16)
      .padStart(2, "0");
  };
  colors[theme] = "#" + channel(0) + channel(8) + channel(4);
}
const output =
  "// Generated from styles/globals.css by scripts/theme-colors.mjs.\nexport const THEME_COLORS = {\n" +
  Object.entries(colors)
    .map(([key, value]) => `  ${key}: "${value}",`)
    .join("\n") +
  "\n} as const;\n";
const path = new URL("../lib/pwa/theme-colors.ts", import.meta.url);
if (process.argv.includes("--check")) {
  if (readFileSync(path, "utf8") !== output)
    throw new Error("Run node scripts/theme-colors.mjs");
} else writeFileSync(path, output);
