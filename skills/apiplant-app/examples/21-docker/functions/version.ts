/**
 * A TypeScript function, because it is the one kind that needs no toolchain in
 * the image: `apiplant build` transpiles it itself and the server runs it in a
 * V8 isolate. A Rust, C, Zig or Go function would need its compiler at image
 * build time — see the README for that variant.
 *
 * The isolate has no access to the process environment, and that is the point:
 * what a deployment wants to vary goes through `version.toml`, whose values are
 * expanded from the environment before the handler ever sees them.
 */

import { config, defineFunctions, s } from "apiplant";

export default defineFunctions({
  version: {
    version: "1.0.0",
    description: "Reports what this container is running.",
    permission: "public",
    method: "GET",
    output: s.object({ release: s.string(), env: s.string() }),

    handler() {
      const { release = "dev", env = "local" } = config<{
        release?: string;
        env?: string;
      }>();

      return { release, env };
    },
  },
});
