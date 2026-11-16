# Forge Website (Astro)

The official Forge site is statically generated with Astro and published at
`https://creature-studio.github.io/forge/` on GitHub Pages.

## Local development

Requires Node.js 22.22+ and npm.

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
so the download stays in sync. The public directory also contains robots.txt.
Astro's sitemap integration generates `sitemap-index.xml` and
`sitemap-0.xml`. All links preserve the `/forge/` base path.

- `src/pages/` owns `/`, `/en/`, `/guide/`, `/en/guide/`, `/releases/`, and `/en/releases/`.
- `src/layouts/Base.astro` provides shared navigation, SEO metadata, canonical
  and language alternatives. `src/components/Landing.astro` implements the
  minimal white-and-blue landing-page design.
- `src/legacy/` holds the previously published documentation and release
  content, preserved as editable HTML until converted to Markdown.
- `src/styles/main.css` contains responsive styles and accessible focus rules.
- `scripts/check-site-seo.py` validates the built output, not the source files.

The site is content-first and ships without a client-side framework or
required JavaScript. Do not replace genuine Forge screenshots with a generated
concept image. Keep GitHub release matrix and download links accurate.
