/**
 * A second source file, which is half of why this function is a directory.
 *
 * A single `.ts` function cannot import anything but `apiplant`, so splitting it
 * up — or reaching for a package — is exactly what the directory form is for.
 */

/** Slugs that would collide with a route this app already serves. */
const RESERVED = new Set(["admin", "api", "auth", "docs", "new", "edit"]);

/** Whether `slug` would shadow something, and so needs a suffix. */
export function isReserved(slug: string): boolean {
  return RESERVED.has(slug);
}
