import { readFileSync, writeFileSync } from "node:fs";
const css = readFileSync(
  new URL("../styles/globals.css", import.meta.url),
  "utf8",
);
// oklch → sRGB hex (Björn Ottosson's reference matrices), so the PWA
// `theme-color` meta carries the exact `--background` of each theme.
function oklchToHex(L, C, H) {
  const a = C * Math.cos((H * Math.PI) / 180);
  const b = C * Math.sin((H * Math.PI) / 180);
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const lin = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
  return (
    "#" +
    lin
      .map((v) => {
        const c = v <= 0.0031308 ? 12.92 * v : 1.055 * v ** (1 / 2.4) - 0.055;
        return Math.round(255 * Math.min(1, Math.max(0, c)))
          .toString(16)
          .padStart(2, "0");
      })
      .join("")
  );
}
const colors = {};
for (const theme of ["dark", "light", "amber"]) {
  const block = css.match(
    new RegExp('\\[data-theme="' + theme + '"\\]\\s*\\{([\\s\\S]*?)\\n\\}'),
  )?.[1];
  const match = block?.match(
    /--background:\s*oklch\(([\d.]+) ([\d.]+) ([\d.]+)\)/,
  );
  if (!match) throw new Error(`Missing ${theme} background`);
  colors[theme] = oklchToHex(
    Number(match[1]),
    Number(match[2]),
    Number(match[3]),
  );
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
