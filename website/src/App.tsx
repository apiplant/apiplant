import { type ParentProps } from "solid-js";
import { createRouter, useLocation } from "@solidjs/router";
import { Header } from "./components/Header";
import { Footer } from "./components/Footer";
import { Home } from "./components/Home";
import { DocsPage, setDrawerOpen } from "./components/Docs";
import { LinkButton } from "./components/ui";

function Shell(props: ParentProps) {
  const location = useLocation();
  const inDocs = () => location.pathname.startsWith("/docs");

  return (
    <div class="flex min-h-screen flex-col">
      {/* The drawer button belongs to the header but the drawer belongs to the
          docs route, so the header only gets the opener. */}
      <Header onToggleSidebar={inDocs() ? () => setDrawerOpen(true) : undefined} />
      <main class="flex-1">{props.children}</main>
      <Footer />
    </div>
  );
}

function NotFound() {
  return (
    <div class="mx-auto w-full max-w-6xl px-5 py-28 text-center">
      <p class="font-mono text-sm text-accent">404</p>
      <h1 class="mt-3 text-3xl font-semibold tracking-tight text-ink">No such page</h1>
      <p class="mx-auto mt-3 max-w-md leading-relaxed text-muted">
        The link may be old, or the guide may have been renamed.
      </p>
      <div class="mt-8 flex justify-center gap-3">
        <LinkButton href="/" variant="primary">
          Home
        </LinkButton>
        <LinkButton href="/docs">Documentation</LinkButton>
      </div>
    </div>
  );
}

const Router = createRouter({
  routes: [
    { path: "/", component: Home },
    // `/docs` renders docs/README.md; `/docs/:slug` renders that guide.
    { path: "/docs", component: DocsPage },
    { path: "/docs/:slug", component: DocsPage },
    { path: "*404", component: NotFound },
  ],
});

export const { paths } = Router;

export function App() {
  return <Router>{(props) => <Shell>{props.children}</Shell>}</Router>;
}
