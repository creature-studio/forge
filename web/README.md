# Forge Website (Astro)

The official Forge site is statically generated with Astro and published at
`https://creature-studio.github.io/forge/` on GitHub Pages.

## Local development

Requires Node.js 22+ and npm.

```sh
cd web
npm ci
npm run dev
npm run build
cd ..
python3 scripts/check-site-seo.py
```

`web/dist/` is the production output and is the only directory deployed.
`web/public/` contains the real terminal screenshots and icons. The root
`install.sh` is copied to `web/public/install.sh` before each production build,
so the download stays in sync. The public directory also contains `robots.txt`.
Astro's sitemap integration generates `sitemap-index.xml` and
`sitemap-0.xml`. All links preserve the `/forge/` base path.

- `src/pages/` owns `/`, `/en/`, `/guide/`, `/en/guide/`, `/releases/`, and `/en/releases/`.
- `src/layouts/Base.astro` provides shared navigation, SEO metadata, canonical
  and language alternatives, syncing the software version directly from the
  workspace `Cargo.toml`.
- `src/components/Landing.astro` and `src/components/Preview.astro` implement
  the full-bleed poster hero, interactive TUI workspace switcher, and cardless
  architectural ledger.
- `src/components/DocsLayout.astro` and `src/components/ReleasesPage.astro`
  power the two-column documentation workspace and changelog timeline.
- `src/styles/main.css` and `src/styles/landing.css` hold the design tokens,
  editorial grid, responsive rules, and accessible focus styles.
- `scripts/check-site-seo.py` validates the built output, links, schema, and
  sitemap.

## Visual identity

The site uses a dark obsidian-and-phosphor-emerald terminal aesthetic
(`#07090d` canvas, `#2ee59d` accent), genuine Forge TUI screenshots, and
tree-shaken **@lucide/astro** SVG components. Do not replace real Forge
screenshots with synthetic mockups. Keep the GitHub release matrix and download
links accurate, and preserve phone-width usability across both languages.
