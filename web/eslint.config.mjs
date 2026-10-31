// eslint pin: 9.x. Bumping to 10.x is blocked on
// `eslint-plugin-react` (pulled in via `eslint-config-next`): the
// 7.37.5 release calls `contextOrFilename.getFilename()` on the
// rule-context arg, which eslint 10 removed. Revisit when
// eslint-plugin-react ships a release that targets the eslint 10
// rule-context API.

import { defineConfig } from "eslint/config";
import nextCoreWebVitals from "eslint-config-next/core-web-vitals";
import nextTypescript from "eslint-config-next/typescript";
import jsxA11y from "eslint-plugin-jsx-a11y";

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
      // `eslint-config-next` already registers the `jsx-a11y` plugin
      // (six rules); the `recommended` preset's rules are spread here
      // rather than extended as a config because its config object
      // re-declares the plugin, and flat config refuses to register the
      // same plugin name twice (next's copy is a CJS interop wrapper,
      // so it is not the same object). The preset adds labels on
      // controls, click handlers paired with key handlers, no positive
      // tabindex, valid roles / ARIA and no handlers on static
      // elements. It is the static half of the a11y gate; the axe sweep
      // in `tests/e2e/a11y.spec.ts` is the rendered half.
      ...jsxA11y.flatConfigs.recommended.rules,
      // The preset's bare `alt-text` would replace next's entry and drop
      // `next/image`'s `<Image>` from the check; keep both the native
      // elements and the component.
      "jsx-a11y/alt-text": [
        "error",
        {
          elements: ["img", "object", "area", 'input[type="image"]'],
          img: ["Image"],
        },
      ],
      // `<ul role="list">` is deliberate: Tailwind's preflight sets
      // `list-style: none`, and Safari / VoiceOver then drop the list
      // semantics unless the role is restated. Every other redundant
      // role is still an error.
      "jsx-a11y/no-redundant-roles": ["error", { ul: ["list"], ol: ["list"] }],
      // Folio's `Input` / `Textarea` wrappers carry `autoFocus` only
      // inside dialogs, popovers and inline editors the user has just
      // opened, where moving focus to the first field is the APG dialog
      // pattern. A raw `<input autoFocus>` is still flagged, so a
      // page-load autofocus cannot land unnoticed.
      "jsx-a11y/no-autofocus": ["error", { ignoreNonDOM: true }],
      // `NativeSelect` renders a native `<select>`; a `<label>` wrapping
      // it labels that control. `depth: 3` lets the label text sit in a
      // nested `<span>` pair (bold name + muted description).
      "jsx-a11y/label-has-associated-control": [
        "error",
        { controlComponents: ["NativeSelect"], depth: 3 },
      ],
      // A scroll container must be focusable to scroll from the keyboard
      // (axe `scrollable-region-focusable`); Folio marks those
      // `role="region"` + `aria-label` + `tabIndex={0}`.
      "jsx-a11y/no-noninteractive-tabindex": [
        "error",
        { roles: ["tabpanel", "region"] },
      ],
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
    // collision padding on popper content, toaster placement, named
    // slider thumbs); app code imports the wrapper so the behaviour is
    // never lost on a refresh.
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
            "slider",
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
