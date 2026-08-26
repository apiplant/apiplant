/// <reference types="vite/client" />

/** The workspace version, substituted at build time (see `vite.config.ts`). */
declare const __APIPLANT_VERSION__: string;

/** The serialised ZBSearch index over `docs/`, built by `build/search-index.ts`. */
declare module "virtual:search-index" {
  const raw: string;
  export default raw;
}

/** The file names in `docs/images/`, listed by `build/docs-images.ts`. */
declare module "virtual:docs-images" {
  const names: string[];
  export default names;
}
