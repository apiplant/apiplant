/// <reference types="vite/client" />

/** The workspace version, substituted at build time (see `vite.config.ts`). */
declare const __APIPLANT_VERSION__: string;

/** The serialised ZBSearch index over `docs/`, built by `build/search-index.ts`. */
declare module "virtual:search-index" {
  const raw: string;
  export default raw;
}
