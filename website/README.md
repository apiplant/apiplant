# apiplant website

The marketing site and the rendered documentation for
[framework.apiplant.com](https://framework.apiplant.com). Solid + Tailwind v4 +
Vite, the same stack and the same palette as [the studio](../studio), which
deploys separately to studio.apiplant.com.

```bash
pnpm install
pnpm dev       # http://127.0.0.1:5274
pnpm build     # → dist/
pnpm check     # types only
```

## The documentation is not copied here

`/docs/<slug>` renders `../docs/<slug>.md`, the repository's own guides, pulled
in raw through Vite's glob import (`@docs/*.md`, aliased in `vite.config.ts`).
There is no second copy to keep in sync, and each guide is its own lazily-loaded
chunk.

To publish a new guide, add `docs/foo.md` and one line to `SECTIONS` in
[`src/lib/docs.ts`](src/lib/docs.ts), which orders and groups the nav. Without
that line the guide still appears under "More", since the nav is built from what
is on disk rather than from the list.

Rendering lives in the same file: markdown-it for the markup, shiki for
highlighting (both themes at once, so the theme toggle recolours code without
re-highlighting), and `markdown-it-anchor` for the heading links the on-page
contents refers to. Relative links between guides, such as
`resources.md#fields`, are rewritten to routes; links outside `docs/` point to
GitHub.

## Styling

`src/app.css` holds the palette and three rules that no utility can express;
everything else is a Tailwind class, including the documentation's reading
theme, which is applied by the markdown-it renderer rules (`DOC_CLASSES`) rather
than by element selectors. The palette matches the studio's variable for
variable, so any change must be made in both places.

## Deploying

A static SPA: `dist/` behind any host, with `public/_redirects` sending unknown
paths to `index.html` so `/docs/functions` resolves on a cold load.
