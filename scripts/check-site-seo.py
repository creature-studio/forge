#!/usr/bin/env python3
"""Validate Astro build output, local links, schema and generated sitemap.

Stdlib-only; run before staging or deploying the static site.
"""
from __future__ import annotations

import json
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import urlsplit, unquote
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
SITE = ROOT / "web" / "dist"
ORIGIN = "https://forge.runs-on.dev"
BASE = ORIGIN + "/"
PAGES = {
    "index.html": {"lang": "zh-CN", "canonical": BASE},
    "en/index.html": {"lang": "en", "canonical": BASE + "en/"},
    "en/guide/index.html": {"lang": "en", "canonical": BASE + "en/guide/"},
    "guide/index.html": {"lang": "zh-CN", "canonical": BASE + "guide/"},
    "releases/index.html": {"lang": "zh-CN", "canonical": BASE + "releases/"},
    "en/releases/index.html": {"lang": "en", "canonical": BASE + "en/releases/"},
    "en/engineering/git-refresh/index.html": {"lang": "en", "canonical": BASE + "en/engineering/git-refresh/"},
}

for entry in json.loads((ROOT / "web/src/data/seo-pages.json").read_text()):
    slug = "en/" + entry["slug"] + "/"
    PAGES[slug + "index.html"] = {"lang": "en", "canonical": BASE + slug}


class Page(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.lang = None
        self.ids = set()
        self.h1 = 0
        self.main = 0
        self.meta = {}
        self.links = []
        self.resources = []
        self.title = ""
        self._title = False
        self._json = False
        self._json_text = ""
        self.schema = []

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag == "html":
            self.lang = a.get("lang")
        if a.get("id"):
            self.ids.add(a["id"])
        if tag == "h1":
            self.h1 += 1
        if tag == "main":
            self.main += 1
        if tag == "title":
            self._title = True
        if tag == "script" and a.get("type") == "application/ld+json":
            self._json = True
            self._json_text = ""
        if tag == "meta":
            self.meta[a.get("name") or a.get("property")] = a.get("content")
        if tag == "link":
            self.links.append(a)
        for field in ("href", "src"):
            if a.get(field):
                self.resources.append(a[field])
        if a.get("srcset"):
            self.resources.extend(part.strip().split()[0] for part in a["srcset"].split(",") if part.strip())

    def handle_data(self, data):
        if self._title:
            self.title += data
        if self._json:
            self._json_text += data

    def handle_endtag(self, tag):
        if tag == "title":
            self._title = False
        if tag == "script" and self._json:
            self.schema.append(json.loads(self._json_text))
            self._json = False


def resolve_local(href: str, source: Path) -> Path | None:
    parts = urlsplit(href)
    if parts.scheme not in ("", "https", "http"):
        return None
    if parts.netloc and parts.netloc != urlsplit(ORIGIN).netloc:
        return None
    if not parts.path:
        return None
    if parts.netloc or parts.path.startswith("/"):
        candidate = SITE / unquote(parts.path.lstrip("/"))
    else:
        candidate = source.parent / unquote(parts.path)
    if candidate.is_dir():
        candidate = candidate / "index.html"
    return candidate.resolve()


def main():
    parsed = {}
    errors = []
    for name, expected in PAGES.items():
        source = SITE / name
        p = Page()
        p.feed(source.read_text(encoding="utf-8"))
        parsed[name] = p
        if p.lang != expected["lang"]:
            errors.append(f"{name}: lang mismatch ({p.lang})")
        if p.h1 != 1 or p.main != 1:
            errors.append(f"{name}: expected one h1 and one main; got {p.h1}/{p.main}")
        if not p.title.strip() or not p.meta.get("description"):
            errors.append(f"{name}: missing title/description")
        canonical = [x["href"] for x in p.links if x.get("rel") == "canonical"]
        if canonical != [expected["canonical"]]:
            errors.append(f"{name}: canonical {canonical}")
        if not p.schema:
            errors.append(f"{name}: missing JSON-LD")
        for ref in p.resources:
            if ref.startswith("https://creature-studio.github.io/forge/") or ref.startswith("/forge/"):
                errors.append(f"{name}: legacy site path {ref}")
            parts = urlsplit(ref)
            if ref.startswith("#") and ref[1:] and ref[1:] not in p.ids:
                errors.append(f"{name}: missing anchor {ref}")
            target = resolve_local(ref, source)
            if target is not None and not target.is_file():
                # install.sh is copied into the deployment from the repository root.
                if not (target == SITE / "install.sh" and (ROOT / "install.sh").is_file()):
                    errors.append(f"{name}: broken link {ref}")
        print(f"OK {name}: title, description, canonical, JSON-LD, links")
    home = parsed["index.html"]
    en = parsed["en/index.html"]
    for name, p in [("zh", home), ("en", en)]:
        alts = {a.get("hreflang"): a.get("href") for a in p.links if a.get("rel") == "alternate"}
        expected = {"en": BASE + "en/", "zh-CN": BASE, "x-default": BASE + "en/"}
        if any(alts.get(k) != v for k, v in expected.items()):
            errors.append(f"{name}: hreflang variants incomplete: {alts}")
    for name in ("index.html", "en/index.html"):
        schemas = parsed[name].schema
        apps = [
            item for schema in schemas
            for item in schema.get("@graph", [schema])
            if item.get("@type") == "SoftwareApplication"
        ]
        # The release metadata must follow the workspace version, not a stale
        # version literal maintained in this validator.
        import tomllib
        version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
        if len(apps) != 1 or apps[0].get("softwareVersion") != version:
            errors.append(f"{name}: wrong software schema version")
    # Keep the previously submitted sitemap.xml usable after the
    # migration to Astro, and ensure it contains the live, full index.
    alias = SITE / "sitemap.xml"
    index = SITE / "sitemap-index.xml"
    if not alias.is_file() or alias.read_bytes() != index.read_bytes():
        errors.append("legacy sitemap.xml missing or not identical to Astro sitemap-index.xml")
    else:
        alias_root = ET.parse(alias).getroot()
        if alias_root.tag != "{http://www.sitemaps.org/schemas/sitemap/0.9}sitemapindex":
            errors.append("legacy sitemap.xml is not a sitemap index")

    sitemap = ET.parse(SITE / "sitemap-0.xml")
    namespace = {"s": "http://www.sitemaps.org/schemas/sitemap/0.9"}
    urls = [x.text for x in sitemap.findall(".//s:url/s:loc", namespace)]
    wanted = {v["canonical"] for v in PAGES.values()}
    if set(urls) != wanted or len(urls) != len(wanted):
        errors.append(f"sitemap entries differ: {urls}")
    if "Sitemap: " + BASE + "sitemap-index.xml" not in (SITE / "robots.txt").read_text():
        errors.append("robots.txt missing sitemap link")
    if errors:
        for error in errors:
            print("ERROR", error)
        raise SystemExit(1)
    print(f"PASS: {len(parsed)} pages, {len(urls)} sitemap entries, reciprocal homepage hreflang")


if __name__ == "__main__":
    main()
