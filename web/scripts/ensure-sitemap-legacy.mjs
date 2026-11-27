import { copyFileSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// Google Search Console has an existing /sitemap.xml submission.
// Astro's sitemap integration emits sitemap-index.xml instead. Preserve
// the original entrypoint as an identical, current sitemap index rather than
// maintaining an independent hand-written list of URLs.
const current = fileURLToPath(new URL('../dist/sitemap-index.xml', import.meta.url));
const legacy = fileURLToPath(new URL('../dist/sitemap.xml', import.meta.url));
const xml = readFileSync(current, 'utf8');
if (!xml.includes('<sitemapindex') || !xml.includes('<loc>')) {
  throw new Error('Astro sitemap index is missing or malformed; refusing to publish a stale alias');
}
copyFileSync(current, legacy);
console.log('Generated /sitemap.xml compatibility alias from sitemap-index.xml');
