import { mkdirSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SITE_URL = (process.env.SITE_URL ?? "https://framework.apiplant.com").replace(/\/$/, "");
const websiteRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const docsRoot = resolve(websiteRoot, "../docs");
const publicRoot = resolve(websiteRoot, "public");

function xml(value) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function routeUrl(pathname) {
  return `${SITE_URL}${pathname === "/" ? "/" : pathname.replace(/\/$/, "")}`;
}

function dateOf(path) {
  return statSync(path).mtime.toISOString().slice(0, 10);
}

const urls = [
  { loc: routeUrl("/"), lastmod: dateOf(resolve(websiteRoot, "src/components/Home.tsx")), changefreq: "weekly", priority: "1.0" },
  { loc: routeUrl("/docs"), lastmod: dateOf(resolve(docsRoot, "README.md")), changefreq: "weekly", priority: "0.9" },
];

for (const filename of readdirSync(docsRoot).sort()) {
  if (!filename.endsWith(".md") || filename === "README.md") continue;
  const slug = filename.replace(/\.md$/, "");
  urls.push({
    loc: routeUrl(`/docs/${slug}`),
    lastmod: dateOf(resolve(docsRoot, filename)),
    changefreq: "monthly",
    priority: "0.8",
  });
}

const body = urls
  .map(
    (url) => `  <url>
    <loc>${xml(url.loc)}</loc>
    <lastmod>${url.lastmod}</lastmod>
    <changefreq>${url.changefreq}</changefreq>
    <priority>${url.priority}</priority>
  </url>`,
  )
  .join("\n");

mkdirSync(publicRoot, { recursive: true });
writeFileSync(
  resolve(publicRoot, "sitemap.xml"),
  `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
${body}
</urlset>
`,
);

console.log(`Wrote ${urls.length} URLs to public/sitemap.xml`);
