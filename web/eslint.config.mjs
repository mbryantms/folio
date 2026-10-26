// eslint pin: 9.x. Bumping to 10.x is blocked on
// `eslint-plugin-react` (pulled in via `eslint-config-next`): the
// 7.37.5 release calls `contextOrFilename.getFilename()` on the
// rule-context arg, which eslint 10 removed. Revisit when
// eslint-plugin-react ships a release that targets the eslint 10
// rule-context API.

import { defineConfig } from "eslint/config";
import nextCoreWebVitals from "eslint-config-next/core-web-vitals";
import nextTypescript from "eslint-config-next/typescript";

export default defineConfig([
  {
    ignores: [
      ".next/**",
      "out/**",
      "node_modules/**",
      "coverage/**",
      "playwright-report/**",
      "test-results/**",
      "lib/api/openapi.json",
      "lib/api/types.ts",
      "next-env.d.ts",
      // serwist emits a minified service-worker bundle here on
      // each production build; it's generated, not source.
      "public/sw.js",
      "public/swe-worker-*.js",
      // Vendored verbatim from the shadcn registry (`shadcn add
      // --overwrite`): never hand-edited, so never linted or formatted.
      "components/ui/**",
      "lib/hooks/**",
    ],
  },
  {
    extends: [...nextCoreWebVitals, ...nextTypescript],
    rules: {
      "@next/next/no-html-link-for-pages": "off",
      // Hygiene
      "no-console": ["warn", { allow: ["warn", "error"] }],
      "prefer-const": "warn",
      "no-debugger": "warn",
      eqeqeq: ["error", "smart"],
      // TypeScript
      "@typescript-eslint/no-unused-vars": [
        "warn",
        {
          argsIgnorePattern: "^_",
          varsIgnorePattern: "^_",
          caughtErrorsIgnorePattern: "^_",
        },
      ],
      "@typescript-eslint/consistent-type-imports": [
        "warn",
        { prefer: "type-imports", fixStyle: "inline-type-imports" },
      ],
      "@typescript-eslint/no-explicit-any": "warn",
    },
  },
  {
    // components/ui is vendored verbatim from the shadcn registry. The
    // modules below have Folio wrappers in components/folio (safe-area
    // collision padding on popper content, toaster placement); app code
    // imports the wrapper so the behaviour is never lost on a refresh.
    ignores: ["components/folio/**", "components/ui/**"],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          paths: [
            "select",
            "dropdown-menu",
            "popover",
            "hover-card",
            "tooltip",
            "tabs",
            "scroll-area",
            "sonner",
            "sidebar",
            "sheet",
          ].map((name) => ({
            name: `@/components/ui/${name}`,
            message: `Import from @/components/folio/${name} (Folio wrapper over the vendored file).`,
          })),
        },
      ],
    },
  },
  {
    files: ["**/*.test.ts", "**/*.test.tsx", "tests/**/*"],
    rules: {
      "no-console": "off",
      "@typescript-eslint/no-explicit-any": "off",
    },
  },
]);
