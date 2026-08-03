/**
 * The console handoff: `#/cli?callback=…`.
 *
 * `apiplant cli` opens a one-request web server on the loopback interface and
 * sends the operator here. This screen issues an API key for the signed-in
 * account and posts it back, so no secret is copied between windows.
 *
 * Two rules make that safe, and both are required:
 *
 * * **The callback must be loopback.** The address arrives in a link, which
 *   anyone can send. Accepting only `127.0.0.1`, `[::1]` and `localhost` over
 *   plain HTTP prevents a crafted link from sending a key to a third party.
 * * **The user must confirm.** Issuing on arrival would make following a link
 *   sufficient to create a credential.
 */

import { Show, createMemo, createSignal } from "solid-js";
import { Button, Card, CardHeader, EmptyState, PageTitle } from "../ui";
import { api, asRecord, notify, reportError } from "../store";

interface Request {
  callback: string;
  name: string;
}

/** Read the handoff parameters out of the address bar. */
function parseRequest(hash: string): Request | null {
  const query = hash.split("?").slice(1).join("?");
  if (!query) return null;
  const params = new URLSearchParams(query);
  const callback = params.get("callback");
  if (!callback || !isLoopback(callback)) return null;
  return { callback, name: params.get("name")?.trim() || "apiplant cli" };
}

/**
 * Whether an address belongs to a program on this machine.
 *
 * Only plain HTTP on a loopback host: the console's listener has no
 * certificate, and no other origin may receive a key.
 */
export function isLoopback(candidate: string): boolean {
  let url: URL;
  try {
    url = new URL(candidate);
  } catch {
    return false;
  }
  if (url.protocol !== "http:") return false;
  const host = url.hostname.replace(/^\[|\]$/g, "");
  return host === "127.0.0.1" || host === "::1" || host === "localhost";
}

export function CliPage() {
  const request = createMemo(() => parseRequest(window.location.hash));
  const [busy, setBusy] = createSignal(false);
  const [issued, setIssued] = createSignal<string | null>(null);
  const [delivered, setDelivered] = createSignal(false);

  const connect = async () => {
    const target = request();
    if (!target) return;
    setBusy(true);
    try {
      const response = asRecord(
        await api("/auth/apikeys", { method: "POST", body: { name: target.name } }),
      );
      const key = typeof response?.api_key === "string" ? response.api_key : null;
      if (!key) throw new Error("The server did not return a key.");
      setIssued(key);

      // `no-cors` with a plain-text body: the console's listener returns
      // permissive headers, but this avoids depending on a preflight and the
      // response is never read.
      await fetch(target.callback, {
        method: "POST",
        mode: "no-cors",
        headers: { "Content-Type": "text/plain" },
        body: key,
      });
      setDelivered(true);
      notify("success", "The console is connected.");
    } catch (error) {
      // The key exists regardless, so displaying it turns a transient failure
      // into a recoverable one rather than a wasted credential.
      reportError(error);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Show
      when={request()}
      fallback={
        <EmptyState
          title="That console link is not usable"
          description="A link from `apiplant cli` carries the address of a listener on your own machine. This one does not, so nothing will be sent."
        />
      }
    >
      {(target) => (
        <>
          <PageTitle
            title="Connect a terminal"
            subtitle="A console on this machine is waiting for a key."
          />

          <Card class="max-w-xl">
            <CardHeader
              title={target().name}
              hint="The key is issued for your account, with your permissions."
            />
            <div class="space-y-4 px-5 py-5 text-sm">
              <p class="text-faint">
                Pressing connect creates an API key and sends it to{" "}
                <code class="rounded bg-black/5 px-1 py-0.5 text-xs dark:bg-white/10">
                  {target().callback}
                </code>
                , a program listening on this computer. Revoke it any time from API keys.
              </p>

              <Show when={delivered()}>
                <p class="text-sm font-medium text-emerald-600 dark:text-emerald-400">
                  Sent. You can close this tab and go back to your terminal.
                </p>
              </Show>

              <Show when={issued() && !delivered()}>
                <div class="space-y-2">
                  <p class="text-xs text-faint">
                    The key was created but could not be delivered; the console may have stopped
                    waiting. Paste it in manually instead:
                  </p>
                  <code class="block break-all rounded bg-black/5 px-3 py-2 text-xs dark:bg-white/10">
                    {issued()}
                  </code>
                </div>
              </Show>

              <Show when={!delivered()}>
                <Button variant="primary" disabled={busy()} onClick={() => void connect()}>
                  {busy() ? "Connecting…" : issued() ? "Try sending again" : "Connect"}
                </Button>
              </Show>
            </div>
          </Card>
        </>
      )}
    </Show>
  );
}
