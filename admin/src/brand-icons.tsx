/**
 * Provider marks for the sign-in buttons.
 *
 * These are [Super Tiny Icons](https://github.com/edent/SuperTinyIcons) — each
 * one a few hundred bytes of hand-written path data, MIT licensed. They are
 * inlined rather than fetched for the reason everything else in this bundle is:
 * the dashboard is a static build that must work against an API on another
 * origin, offline, and with no third-party request to a CDN that would tell
 * somebody else who is signing in to what.
 *
 * Each icon draws its own background — Google's white, LinkedIn's blue —
 * because these are trademarks, and their brand guidelines ask for the mark as
 * they drew it rather than recoloured to match a host page. So a Google button
 * looks the same in dark mode, on purpose.
 *
 * GitHub is the exception, and only because GitHub publishes two marks: a black
 * one and a white one for dark backgrounds. Ours carries the two paths as
 * classes so the stylesheet can pick, since a black tile on this theme's dark
 * surface is a button with no logo on it.
 *
 * A provider apiplant does not ship a mark for falls back to [`InitialMark`],
 * which is its first letter on a neutral tile. That keeps a `[oauth.gitlab]`
 * block working with no edit here — the row simply looks plainer than the four
 * below.
 */

import type { JSX } from "@solidjs/web";

/** Every mark is square and sized by its container. */
function frame(children: JSX.Element, label: string, extra = ""): JSX.Element {
  return (
    <svg
      viewBox="0 0 512 512"
      class={`h-[1.125rem] w-[1.125rem] shrink-0 rounded-[3px] ${extra}`}
      role="img"
      aria-label={label}
    >
      {children}
    </svg>
  );
}

export function GitHubMark() {
  // The two paths carry classes rather than fills: which way round they go is a
  // question about the page behind them, and the stylesheet is where this
  // dashboard answers those. See `.mark-github` in the shared CSS.
  return frame(
    <>
      <path class="mark-bg" d="m0 0H512V512H0" />
      <path
        class="mark-fg"
        d="M335 499c-13 0-16-6-16-12l1-70c0-24-8-40-18-48 57-6 117-28 117-126 0-28-10-51-26-69 3-6 11-32-3-67 0 0-21-7-70 26-42-12-86-12-128 0-49-33-70-26-70-26-14 35-6 61-3 67-16 18-26 41-26 69 0 98 59 120 116 126-7 7-14 18-16 35-15 6-52 17-74-22 0 0-14-24-40-26 0 0-25 0-1 16 0 0 16 7 28 37 0 0 15 50 86 34l1 44c0 6-3 12-16 12-14 0-12 17-12 17H347s2-17-12-17Z"
      />
    </>,
    "GitHub",
    "mark-github",
  );
}

export function GoogleMark() {
  return frame(
    <>
      <path d="m0 0H512V512H0" fill="#fff" />
      <path fill="#34a853" d="M153 292c30 82 118 95 171 60h62v48A192 192 0 0190 341" />
      <path fill="#4285f4" d="m386 400a140 175 0 0053-179H260v74h102q-7 37-38 57" />
      <path fill="#fbbc02" d="m90 341a208 200 0 010-171l63 49q-12 37 0 73" />
      <path fill="#ea4335" d="m153 219c22-69 116-109 179-50l55-54c-78-75-230-72-297 55" />
    </>,
    "Google",
  );
}

export function LinkedInMark() {
  return frame(
    <>
      <path d="m0 0H512V512H0" fill="#0077b5" />
      <g fill="#fff">
        <circle cx="142" cy="138" r="37" />
        <path stroke="#fff" stroke-width="66" d="M244 194v198M142 194v198" />
        <path d="M276 282c0-20 13-40 36-40 24 0 33 18 33 45v105h66V279c0-61-32-89-76-89-34 0-51 19-59 32" />
      </g>
    </>,
    "LinkedIn",
  );
}

export function XMark() {
  return frame(
    <>
      <rect width="512" height="512" fill="#fff" />
      <path d="M321.8 373.1h36.6L190 137.5H153.4ZM391 389.9H310.6L237 285.1 144.8 389.9H121L226.4 270 121 120h80.4l69.7 99.2L358.4 120h23.8L281.7 234.3Z" />
    </>,
    "X",
  );
}

/**
 * An image the app supplied — `[oauth.<provider>] icon`, usually a file in its
 * `public/` directory.
 *
 * Squared and rounded like the drawn marks so a row of buttons stays a row of
 * buttons whatever the file is. It is loaded from the app's own origin: this is
 * the one mark apiplant did not draw, and it should still not be a request to
 * somebody else's CDN.
 */
export function ImageMark(props: { src: string; label: string }) {
  return (
    <img
      src={props.src}
      alt=""
      aria-hidden="true"
      class="h-[1.125rem] w-[1.125rem] shrink-0 rounded-[3px] object-contain"
    />
  );
}

/**
 * The fallback: a letter on a neutral tile, for a provider configured by an app
 * that apiplant ships no mark for.
 */
export function InitialMark(props: { label: string }) {
  const letter = () => (props.label.trim()[0] ?? "?").toUpperCase();
  return (
    <span
      class="flex h-[1.125rem] w-[1.125rem] shrink-0 items-center justify-center rounded-[3px] bg-surface-3 text-[0.625rem] font-semibold text-muted"
      aria-hidden="true"
    >
      {letter()}
    </span>
  );
}

/**
 * The mark for a provider, by the name it is configured under.
 *
 * In order: the four apiplant draws, then whatever image the app configured,
 * then the provider's initial. A configured `icon` deliberately does *not*
 * override the built-in four — those are trademarks, drawn to their owners'
 * guidelines, and an app that wants a different GitHub logo is an app about to
 * get its sign-in button wrong.
 *
 * `twitter` is here because an app that had `[oauth.twitter]` before the rename
 * should not lose its logo over it.
 */
export function ProviderMark(props: { provider: string; label: string; icon?: string }) {
  switch (props.provider.toLowerCase()) {
    case "github":
      return <GitHubMark />;
    case "google":
      return <GoogleMark />;
    case "linkedin":
      return <LinkedInMark />;
    case "x":
    case "twitter":
      return <XMark />;
    default:
      return props.icon ? (
        <ImageMark src={props.icon} label={props.label} />
      ) : (
        <InitialMark label={props.label} />
      );
  }
}
