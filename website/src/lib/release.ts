/**
 * The published release and its assets.
 *
 * `release.yml` builds one archive per target and names it
 * `apiplant-<tag>-<target>.tar.gz`, so a direct link can be assembled here
 * rather than sending every reader to the releases page to hunt for their own.
 * The version comes from the workspace manifest at build time, which means a
 * site deployed from a commit predating the tag would link at an asset that
 * does not exist yet — deploy the site after the release, not before.
 */

import { GITHUB_URL } from "./links";

export const VERSION = __APIPLANT_VERSION__;
/** Git tags are `v`-prefixed, and the tag is part of every asset name. */
export const TAG = `v${VERSION}`;

export const RELEASES_URL = `${GITHUB_URL}/releases`;
export const LATEST_RELEASE_URL = `${RELEASES_URL}/tag/${TAG}`;

export interface Platform {
  /** Rust target triple, as it appears in the asset name. */
  target: string;
  label: string;
  /** For the download button, which cannot wrap and sits in a narrow column. */
  short: string;
}

/* Exactly the targets `release.yml` builds. macOS on Intel is deliberately not
   among them, so an Intel Mac falls through to the releases page rather than
   being offered an arm64 binary it cannot run. */
export const PLATFORMS: Platform[] = [
  { target: "aarch64-apple-darwin", label: "macOS · Apple silicon", short: "Apple silicon" },
  { target: "x86_64-unknown-linux-gnu", label: "Linux · x86_64", short: "Linux x86_64" },
  { target: "aarch64-unknown-linux-gnu", label: "Linux · aarch64", short: "Linux aarch64" },
];

export function assetName(platform: Platform): string {
  return `apiplant-${TAG}-${platform.target}.tar.gz`;
}

export function downloadUrl(platform: Platform): string {
  return `${RELEASES_URL}/download/${TAG}/${assetName(platform)}`;
}

/** `navigator.userAgentData`, which TypeScript's DOM lib does not describe. */
interface UserAgentData {
  platform: string;
  getHighEntropyValues(hints: string[]): Promise<{ architecture?: string }>;
}

/**
 * The visitor's platform, or `null` when it is one nothing is published for
 * (Windows, an Intel Mac, a phone) and the releases page is the honest answer.
 *
 * Asynchronous because the user-agent string is not enough: Chrome on Linux
 * reports `X11; Linux x86_64` whatever the machine is, so an aarch64 visitor
 * would be handed an x86_64 archive. `getHighEntropyValues` is the only way to
 * read the real architecture, and it returns a promise.
 */
export async function detectPlatform(): Promise<Platform | null> {
  const data = (navigator as Navigator & { userAgentData?: UserAgentData }).userAgentData;
  const ua = navigator.userAgent;

  if (data) {
    const architecture = await data
      .getHighEntropyValues(["architecture"])
      .then((values) => values.architecture)
      .catch(() => undefined);

    if (architecture) {
      const platform = data.platform.toLowerCase();
      // Client hints spell the two architectures `arm` and `x86`, and report a
      // 64-bit ARM machine as `arm` — there is no `arm64` value.
      if (platform.includes("linux")) {
        return find(architecture === "arm" ? "aarch64-unknown-linux-gnu" : "x86_64-unknown-linux-gnu");
      }
      // An Intel Mac reports `x86`, and no darwin x86_64 archive is built.
      if (platform.includes("mac")) {
        return architecture === "arm" ? find("aarch64-apple-darwin") : null;
      }
      return null;
    }
  }

  // Safari and Firefox have no client hints, leaving only the user-agent. An
  // Apple silicon Mac is indistinguishable from an Intel one there (both say
  // `Intel Mac OS X`), and arm64 is now the overwhelmingly likelier machine,
  // so it is what gets offered.
  if (/Mac OS X/.test(ua) && !/iPhone|iPad/.test(ua)) return find("aarch64-apple-darwin");
  if (/Linux/.test(ua) && !/Android/.test(ua)) {
    return find(/aarch64|arm64/i.test(ua) ? "aarch64-unknown-linux-gnu" : "x86_64-unknown-linux-gnu");
  }
  return null;
}

function find(target: string): Platform | null {
  return PLATFORMS.find((platform) => platform.target === target) ?? null;
}
