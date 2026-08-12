import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SITE_URL = (process.env.SITE_URL ?? "https://framework.apiplant.com").replace(/\/$/, "");
const websiteRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const docsRoot = resolve(websiteRoot, "../docs");
const distRoot = resolve(websiteRoot, "dist");
const template = readFileSync(resolve(distRoot, "index.html"), "utf8");
const image = `${SITE_URL}/og-image.png`;

function routeUrl(pathname) {
  return `${SITE_URL}${pathname === "/" ? "/" : pathname.replace(/\/$/, "")}`;
}

function escapeAttr(value) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll('"', "&quot;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function textFromMarkdown(markdown) {
  return markdown
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/!\[[^\]]*]\([^)]+\)/g, " ")
    .replace(/\[([^\]]+)]\([^)]+\)/g, "$1")
    .replace(/[*_>#~-]/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function excerpt(text, length = 155) {
  if (text.length <= length) return text;
  const clipped = text.slice(0, length + 1);
  const boundary = clipped.lastIndexOf(" ");
  return `${clipped.slice(0, boundary > 80 ? boundary : length).trimEnd()}...`;
}

function docMeta(filename) {
  const markdown = readFileSync(resolve(docsRoot, filename), "utf8");
  const title = /^#\s+(.+)$/m.exec(markdown)?.[1]?.trim() ?? "apiplant documentation";
  const afterTitle = markdown.replace(/^#\s+.+$/m, "");
  const paragraph = afterTitle
    .split(/\n\s*\n/)
    .map(textFromMarkdown)
    .find((candidate) => candidate.length > 40);
  const description =
    paragraph
      ? excerpt(paragraph)
      : "Read the apiplant framework documentation for configuration, resources, permissions, functions, services and operations.";
  return { title, description };
}

function replaceTag(html, selector, tag) {
  const pattern =
    selector.kind === "title"
      ? /<title>[\s\S]*?<\/title>/
      : new RegExp(
          `<meta\\s+${selector.kind}=["']${selector.name}["'][^>]*>`,
          "i",
        );
  return pattern.test(html) ? html.replace(pattern, tag) : html.replace("</head>", `    ${tag}\n  </head>`);
}

function replaceLink(html, rel, href) {
  const tag = `<link rel="${rel}" href="${escapeAttr(href)}" />`;
  const pattern = new RegExp(`<link\\s+rel=["']${rel}["'][^>]*>`, "i");
  return pattern.test(html) ? html.replace(pattern, tag) : html.replace("</head>", `    ${tag}\n  </head>`);
}

function pageHtml({ pathname, title, description, type = "article" }) {
  const fullTitle = title.includes("apiplant") ? title : `${title} — apiplant docs`;
  const url = routeUrl(pathname);
  let html = template;
  html = replaceTag(html, { kind: "title" }, `<title>${escapeAttr(fullTitle)}</title>`);
  html = replaceTag(html, { kind: "name", name: "description" }, `<meta name="description" content="${escapeAttr(description)}" />`);
  html = replaceTag(html, { kind: "name", name: "robots" }, `<meta name="robots" content="index, follow" />`);
  html = replaceLink(html, "canonical", url);
  html = replaceTag(html, { kind: "property", name: "og:title" }, `<meta property="og:title" content="${escapeAttr(fullTitle)}" />`);
  html = replaceTag(html, { kind: "property", name: "og:description" }, `<meta property="og:description" content="${escapeAttr(description)}" />`);
  html = replaceTag(html, { kind: "property", name: "og:url" }, `<meta property="og:url" content="${escapeAttr(url)}" />`);
  html = replaceTag(html, { kind: "property", name: "og:type" }, `<meta property="og:type" content="${type}" />`);
  html = replaceTag(html, { kind: "property", name: "og:image" }, `<meta property="og:image" content="${escapeAttr(image)}" />`);
  html = replaceTag(html, { kind: "name", name: "twitter:title" }, `<meta name="twitter:title" content="${escapeAttr(fullTitle)}" />`);
  html = replaceTag(html, { kind: "name", name: "twitter:description" }, `<meta name="twitter:description" content="${escapeAttr(description)}" />`);
  html = replaceTag(html, { kind: "name", name: "twitter:image" }, `<meta name="twitter:image" content="${escapeAttr(image)}" />`);
  return html;
}

function writeRoute(pathname, html) {
  const dir = pathname === "/" ? distRoot : resolve(distRoot, pathname.slice(1));
  mkdirSync(dir, { recursive: true });
  writeFileSync(resolve(dir, "index.html"), html);
}

const routes = [];
const docsIndex = docMeta("README.md");
routes.push({ pathname: "/docs", ...docsIndex });

for (const filename of readdirSync(docsRoot).sort()) {
  if (!filename.endsWith(".md") || filename === "README.md") continue;
  const slug = filename.replace(/\.md$/, "");
  routes.push({ pathname: `/docs/${slug}`, ...docMeta(filename) });
}

for (const route of routes) {
  writeRoute(route.pathname, pageHtml(route));
}

console.log(`Wrote ${routes.length} route-specific HTML files`);
