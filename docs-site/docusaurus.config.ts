import {themes as prismThemes} from 'prism-react-renderer';
import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';
import type * as OpenApiPlugin from 'docusaurus-plugin-openapi-docs';

const GITHUB_REPO = 'https://github.com/mbryantms/folio';

const config: Config = {
  title: 'Folio',
  tagline: 'Self-hostable comic reader',
  favicon: 'img/favicon.ico',

  future: {
    v4: true,
  },

  url: 'https://folio.example.com',
  baseUrl: '/',

  organizationName: 'mbryantms',
  projectName: 'folio',

  // Feature-showcase pages cross-link to sibling feature pages
  // (markers, library, etc.) that haven't been authored yet.
  // Restore to 'throw' once every page in the showcase has shipped
  // so drift in cross-links fails the build. Image placeholders use
  // :::info admonitions rather than markdown image refs, so the
  // onBrokenMarkdownImages default ('throw') stays in effect.
  onBrokenLinks: 'warn',
  markdown: {
    hooks: {
      onBrokenMarkdownLinks: 'warn',
    },
  },

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      {
        docs: {
          sidebarPath: './sidebars.ts',
          editUrl: `${GITHUB_REPO}/edit/main/docs-site/`,
          docItemComponent: '@theme/ApiItem',
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  plugins: [
    [
      'docusaurus-plugin-openapi-docs',
      {
        id: 'openapi',
        docsPluginId: 'classic',
        config: {
          folio: {
            specPath: '../web/lib/api/openapi.json',
            outputDir: 'docs/api',
            sidebarOptions: {
              groupPathsBy: 'tag',
              categoryLinkSource: 'tag',
            },
            // The "Try it" send button is hidden because the spec has no
            // `servers` block and no `proxy` is configured here. Don't add
            // either in production — pasted auth headers would leak into
            // the static build. If you need a live tester, run a dev build
            // with a local-only servers entry.
          } satisfies OpenApiPlugin.Options,
        },
      },
    ],
  ],

  themes: ['docusaurus-theme-openapi-docs'],

  themeConfig: {
    image: 'img/docusaurus-social-card.jpg',
    colorMode: {
      respectPrefersColorScheme: true,
    },
    navbar: {
      title: 'Folio',
      logo: {
        alt: 'Folio',
        src: 'img/logo.svg',
      },
      items: [
        {
          type: 'docSidebar',
          sidebarId: 'docs',
          position: 'left',
          label: 'Docs',
        },
        {
          type: 'docSidebar',
          sidebarId: 'api',
          position: 'left',
          label: 'API',
        },
        {
          href: GITHUB_REPO,
          label: 'GitHub',
          position: 'right',
        },
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Docs',
          items: [
            {label: 'Introduction', to: '/docs/intro'},
            {label: 'Architecture', to: '/docs/architecture/overview'},
            {label: 'Operations', to: '/docs/operations/deploy'},
            {label: 'Contributing', to: '/docs/contributing/setup'},
          ],
        },
        {
          title: 'Project',
          items: [
            {label: 'GitHub', href: GITHUB_REPO},
            {label: 'Issues', href: `${GITHUB_REPO}/issues`},
          ],
        },
      ],
      copyright: `Copyright © ${new Date().getFullYear()} Folio contributors. Built with Docusaurus.`,
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.dracula,
      additionalLanguages: ['rust', 'toml', 'bash', 'json'],
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
