/**
 * Billing, for the organisation you are currently in.
 *
 * The `billing_*` tables are ordinary resources with generic screens already.
 * This screen exists for the two operations a table of rows cannot perform,
 * starting a checkout and opening the provider's portal, and to summarise the
 * current plan and renewal date, which a table presents poorly.
 *
 * Everything here is read through the ordinary API with the ordinary
 * permissions. The price list is public, the subscription is readable by any
 * member, and the payment history and the two buttons require `role:admin`, so
 * a member sees their plan and an admin sees the payment details.
 */

import { For, Show, createMemo, createResource, createSignal } from "solid-js";
import { Badge, Button, Card, CardHeader, EmptyState, PageTitle } from "../ui";
import {
  api,
  asRecord,
  asRecords,
  currentOrganization,
  hasRole,
  manifest,
  notify,
  organizationLabel,
  reportError,
} from "../store";
import type { ApiRecord } from "../types";

/** Statuses that mean the organisation is entitled to what it pays for. */
const ENTITLED = ["active", "trialing"];

/** How a status reads to somebody who does not work at a payment provider. */
const STATUS_LABEL: Record<string, string> = {
  active: "Active",
  trialing: "Trial",
  past_due: "Payment overdue",
  canceled: "Cancelled",
  unpaid: "Unpaid",
  incomplete: "Not finished",
  incomplete_expired: "Expired before it started",
  paused: "Paused",
};

/**
 * An amount in the smallest unit, as money.
 *
 * Prices are stored as integers because that is the only exact representation,
 * so 1000 is €10.00, and this is the one place that converts back. `Intl` is
 * given the minor units so a currency without them, such as JPY, is not shown
 * as ¥10.00 for a ¥1000 charge.
 */
function money(amount: number, currency: string): string {
  const code = (currency || manifest()?.billing?.currency || "usd").toUpperCase();
  try {
    const format = new Intl.NumberFormat(undefined, { style: "currency", currency: code });
    const digits = format.resolvedOptions().maximumFractionDigits ?? 2;
    return format.format(amount / 10 ** digits);
  } catch {
    // An unknown currency code is still shown as a number rather than omitted.
    return `${(amount / 100).toFixed(2)} ${code}`;
  }
}

/** "Monthly", "Every 3 months", "One-off". */
function cadence(price: ApiRecord): string {
  const interval = String(price.interval ?? "").trim();
  if (!interval) return "One-off";
  const count = Number(price.interval_count ?? 1) || 1;
  if (count === 1) {
    return { day: "Daily", week: "Weekly", month: "Monthly", year: "Yearly" }[interval] ?? interval;
  }
  return `Every ${count} ${interval}s`;
}

function when(value: unknown): string {
  if (typeof value !== "string" || !value) return "—";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "—" : date.toLocaleDateString();
}

export function BillingPage() {
  const [busy, setBusy] = createSignal<string | null>(null);
  const billing = createMemo(() => manifest()?.billing ?? null);
  const org = createMemo(() => currentOrganization());
  const isAdmin = createMemo(() => hasRole("admin"));

  // Public: the price list is readable without an account, so it loads whether
  // or not an organisation is selected.
  const [prices] = createResource(async () =>
    asRecords(await api("/billing_price?active=true&expand=product&limit=100")),
  );

  const [subscription] = createResource(
    () => org()?.id ?? null,
    async () => {
      const rows = asRecords(await api("/billing_subscription?limit=5"));
      // The active paying subscription, if any; an organisation that has
      // changed plans has older rows alongside the current one.
      return rows.find((row) => ENTITLED.includes(String(row.status ?? ""))) ?? rows[0] ?? null;
    },
  );

  const [payments] = createResource(
    () => (isAdmin() ? org()?.id ?? null : null),
    async () => asRecords(await api("/billing_payment?limit=10&sort=-created_at")),
  );

  const subscribe = async (price: ApiRecord) => {
    const id = String(price.id ?? "");
    setBusy(id);
    try {
      const response = asRecord(
        await api("/billing/checkout", { method: "POST", body: { price_id: id } }),
      );
      const url = typeof response?.url === "string" ? response.url : "";
      if (!url) throw new Error("the checkout came back with no URL");
      // A full navigation rather than a new tab: the provider's page continues
      // this flow and returns here when finished.
      window.location.assign(url);
    } catch (error) {
      reportError(error);
      setBusy(null);
    }
  };

  const openPortal = async () => {
    setBusy("portal");
    try {
      const response = asRecord(await api("/billing/portal", { method: "POST", body: {} }));
      const url = typeof response?.url === "string" ? response.url : "";
      if (!url) throw new Error("the portal came back with no URL");
      window.location.assign(url);
    } catch (error) {
      reportError(error);
      setBusy(null);
    }
  };

  return (
    <>
      <PageTitle
        title="Billing"
        subtitle={`What ${organizationLabel(org())} pays for, and how.`}
      >
        <Show when={isAdmin() && subscription()}>
          <Button variant="secondary" loading={busy() === "portal"} onClick={() => void openPortal()}>
            Manage payment details
          </Button>
        </Show>
      </PageTitle>

      {/* The one misconfiguration that looks like silence: checkouts complete,
          the customer is charged, and nothing is ever written down. */}
      <Show when={billing() && !billing()!.webhooks_configured && isAdmin()}>
        <Card class="mb-4">
          <div class="px-5 py-4">
            <p class="text-sm font-medium">Webhooks are not configured</p>
            <p class="mt-1 text-xs text-faint">
              Purchases will go through and be charged, but no subscription or payment will be
              recorded here. Set <code>[payments] webhook_secret</code> in <code>main.toml</code>{" "}
              and point the provider at <code>/billing/webhook</code>.
            </p>
          </div>
        </Card>
      </Show>

      <div class="grid gap-4 xl:grid-cols-2">
        <Card>
          <CardHeader title="Current plan" />
          <Show
            when={org()}
            fallback={
              <div class="px-5 py-4">
                <EmptyState
                  title="No organization selected"
                  description="Billing belongs to an organization. Choose one from the top bar."
                />
              </div>
            }
          >
            <Show
              when={subscription()}
              fallback={
                <div class="px-5 py-4">
                  <EmptyState
                    title="Nothing subscribed"
                    description={
                      isAdmin()
                        ? "Choose a plan below to get started."
                        : "An admin of this organization can start a subscription."
                    }
                  />
                </div>
              }
            >
              {(row) => (
                <dl class="space-y-3 px-5 py-5 text-sm">
                  <div class="flex items-center justify-between gap-3">
                    <dt class="text-faint">Status</dt>
                    <dd>
                      <Badge
                        tone={ENTITLED.includes(String(row().status ?? "")) ? "success" : "warn"}
                      >
                        {STATUS_LABEL[String(row().status ?? "")] ?? String(row().status ?? "")}
                      </Badge>
                    </dd>
                  </div>
                  <div class="flex items-center justify-between gap-3">
                    <dt class="text-faint">
                      {row().cancel_at_period_end ? "Access ends" : "Renews"}
                    </dt>
                    <dd>{when(row().current_period_end)}</dd>
                  </div>
                  <Show when={row().trial_ends_at}>
                    <div class="flex items-center justify-between gap-3">
                      <dt class="text-faint">Trial ends</dt>
                      <dd>{when(row().trial_ends_at)}</dd>
                    </div>
                  </Show>
                  <Show when={row().cancel_at_period_end}>
                    <p class="text-xs text-faint">
                      This subscription has been cancelled. It stays active until the date above —
                      the period it has already paid for.
                    </p>
                  </Show>
                </dl>
              )}
            </Show>
          </Show>
        </Card>

        <Card>
          <CardHeader
            title="Plans"
            hint={
              billing()?.automatic_tax ? "Prices exclude tax, which is added at checkout" : undefined
            }
          />
          <Show
            when={(prices() ?? []).length}
            fallback={
              <div class="px-5 py-4">
                <EmptyState
                  title="Nothing on sale yet"
                  description="Add a product and a price, and they will appear here."
                />
              </div>
            }
          >
            <ul class="divide-y divide-line">
              <For each={prices() ?? []}>
                {(price) => {
                  const product = asRecord(price.product);
                  const current = () =>
                    String(subscription()?.price_id ?? "") === String(price.id ?? "");
                  return (
                    <li class="flex items-center justify-between gap-4 px-5 py-4">
                      <div class="min-w-0">
                        <p class="truncate text-sm font-medium">
                          {String(product?.name ?? price.nickname ?? "Plan")}
                        </p>
                        <p class="truncate text-xs text-faint">
                          {money(Number(price.unit_amount ?? 0), String(price.currency ?? ""))} ·{" "}
                          {cadence(price)}
                          <Show when={Number(price.trial_days ?? 0) > 0}>
                            {` · ${Number(price.trial_days)}-day trial`}
                          </Show>
                        </p>
                      </div>
                      <Show
                        when={isAdmin() && !current()}
                        fallback={
                          <Show when={current()}>
                            <Badge tone="success">Current</Badge>
                          </Show>
                        }
                      >
                        <Button
                          variant="primary"
                          loading={busy() === String(price.id ?? "")}
                          onClick={() => void subscribe(price)}
                        >
                          {subscription() ? "Switch" : "Choose"}
                        </Button>
                      </Show>
                    </li>
                  );
                }}
              </For>
            </ul>
          </Show>
        </Card>

        <Show when={isAdmin()}>
          <Card class="xl:col-span-2">
            <CardHeader title="Payments" hint="What has been charged, and whether it went through" />
            <Show
              when={(payments() ?? []).length}
              fallback={<p class="px-5 py-4 text-xs text-faint">Nothing has been charged yet.</p>}
            >
              <ul class="divide-y divide-line">
                <For each={payments() ?? []}>
                  {(payment) => (
                    <li class="flex items-center justify-between gap-4 px-5 py-3 text-sm">
                      <div class="min-w-0">
                        <p class="truncate">
                          {money(Number(payment.amount ?? 0), String(payment.currency ?? ""))}
                          <Show when={Number(payment.tax_amount ?? 0) > 0}>
                            <span class="text-xs text-faint">
                              {` (incl. ${money(
                                Number(payment.tax_amount),
                                String(payment.currency ?? ""),
                              )} tax)`}
                            </span>
                          </Show>
                        </p>
                        <p class="truncate text-xs text-faint">
                          {when(payment.paid_at ?? payment.created_at)}
                          <Show when={payment.description}>{` · ${String(payment.description)}`}</Show>
                        </p>
                      </div>
                      <div class="flex shrink-0 items-center gap-3">
                        <Badge tone={payment.status === "succeeded" ? "success" : "warn"}>
                          {String(payment.status ?? "")}
                        </Badge>
                        <Show when={typeof payment.receipt_url === "string" && payment.receipt_url}>
                          <a
                            class="text-xs underline"
                            href={String(payment.receipt_url)}
                            target="_blank"
                            rel="noreferrer"
                          >
                            Receipt
                          </a>
                        </Show>
                      </div>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </Card>
        </Show>
      </div>
    </>
  );
}

/**
 * Notice the outcome the provider sent the buyer back with, once.
 *
 * The redirect lands on `#/billing?checkout=success`, which is a claim made by
 * the browser rather than a confirmed fact: the webhook records the purchase and
 * may not have arrived yet. This screen therefore acknowledges the return and
 * refetches, without treating anything as paid.
 */
export function noticeCheckoutOutcome() {
  const query = window.location.hash.split("?")[1] ?? "";
  const outcome = new URLSearchParams(query).get("checkout");
  if (!outcome) return;
  if (outcome === "success") {
    notify("success", "Thank you. Your payment went through, and may take a moment to appear here.");
  } else {
    notify("info", "Checkout cancelled. Nothing has been charged.");
  }
  window.history.replaceState(null, "", "#/billing");
}
