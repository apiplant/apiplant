/**
 * The two screens the deployment's own administrators get, and nobody else:
 * every organisation, and every account.
 *
 * The rest of the dashboard is written from inside one organisation. A global
 * admin's questions are the other shape ("which tenant is this person in",
 * "who is in that one"), and these two screens ask the API the same questions a
 * support conversation does.
 *
 * Both are gated on {@link isGlobalAdmin} rather than on a policy: the server
 * lifts role and organisation checks for exactly these callers, so what makes
 * the screens work is the same thing that makes them appear.
 *
 * Neither screen filters anything by default; the search and drop-downs narrow
 * it, and the pager keeps "everything" a page long.
 */

import { For, Show, createEffect, createMemo, createSignal, isPending, latest, refresh } from "solid-js";
import { Avatar, Badge, Button, Card, CardHeader, EmptyState, PageTitle, SearchInput } from "../ui";
import {
  api,
  asRecord,
  asRecords,
  avatarOf,
  emailOf,
  impersonate,
  isGlobalAdmin,
  manifest,
  mayImpersonate,
  navigate,
  notify,
  organizationLabel,
  refreshSession,
  reportError,
  session,
  setActiveOrganization,
} from "../store";
import type { ApiRecord } from "../types";

/** Rows per page. Both screens open on the unfiltered list, so this is what
 *  keeps a deployment with ten thousand accounts readable. */
const PAGE_SIZE = 50;

/** Shown where a screen exists but this caller is not the back office. */
function NotTheBackOffice() {
  return (
    <Card>
      <div class="px-5 py-4">
        <EmptyState
          title="Not available"
          description="This screen belongs to the deployment's administrators, named by `[organization] global_admin_role`."
        />
      </div>
    </Card>
  );
}

/**
 * Previous/next for a list whose endpoint returns no total.
 *
 * One row more than fits is fetched, and its presence is the whole of "there
 * is a next page" — the same trick the resource tables use, and the reason a
 * pager can be right without a count behind it.
 */
function Pager(props: { page: number; hasNext: boolean; onPage: (page: number) => void }) {
  return (
    <Show when={props.page > 0 || props.hasNext}>
      <div class="flex items-center justify-between border-t border-line px-5 py-3 text-xs text-faint">
        <span>Page {props.page + 1}</span>
        <div class="flex gap-2">
          <Button size="sm" variant="ghost" disabled={props.page === 0} onClick={() => props.onPage(props.page - 1)}>
            Previous
          </Button>
          <Button size="sm" variant="ghost" disabled={!props.hasNext} onClick={() => props.onPage(props.page + 1)}>
            Next
          </Button>
        </div>
      </div>
    </Show>
  );
}

// --- organizations ----------------------------------------------------------

export function OrganizationsPage() {
  const [search, setSearch] = createSignal("");
  const [applied, setApplied] = createSignal("");
  const [orgClass, setOrgClass] = createSignal("");
  const [page, setPage] = createSignal(0);

  // Narrowing a list starts it again at the first page: page 3 of a list that
  // no longer has three pages is an empty screen with no explanation on it.
  createEffect(
    () => [applied(), orgClass()] as const,
    () => {
      setPage(0);
    },
  );

  const rowsResource = createMemo(async () => {
    const key = { search: applied(), orgClass: orgClass(), page: page(), admin: isGlobalAdmin() };
    if (!key.admin) return [];
    const params = new URLSearchParams({
      limit: String(PAGE_SIZE + 1),
      offset: String(key.page * PAGE_SIZE),
      order: "name",
    });
    if (key.search) params.set("search", key.search);
    if (key.orgClass) params.set("org_class", key.orgClass);
    // No `org: true`: this caller is the back office wherever they stand, and
    // sending the header would narrow a deployment-wide list to one tenant.
    return asRecords(await api(`/organization?${params.toString()}`));
  });
  const rows = () => latest(rowsResource) ?? [];
  const loading = () => isPending(rowsResource);
  const refetch = () => refresh(rowsResource);
  const visible = createMemo(() => rows().slice(0, PAGE_SIZE));
  const hasNext = createMemo(() => rows().length > PAGE_SIZE);

  // What the app's permissions mention, plus whatever the rows on screen
  // actually carry: a class is a free string, so a deployment can be using one
  // no policy has been written against yet and it still has to be filterable.
  const classes = createMemo(() => {
    const known = new Set(manifest()?.organization?.known_classes ?? []);
    for (const row of rows()) if (typeof row.org_class === "string" && row.org_class) known.add(row.org_class);
    return [...known].sort();
  });

  const setClass = async (id: string, value: string) => {
    try {
      await api(`/organization/${encodeURIComponent(id)}`, {
        method: "PATCH",
        body: { org_class: value || null },
      });
      await refreshSession();
      refetch();
      notify("success", value ? `Class set to ${value}.` : "Class cleared.");
    } catch (error) {
      reportError(error);
    }
  };

  /** Make this the active organisation, then open its team. */
  const openTeam = async (id: string) => {
    if (session.organizationId !== id) await setActiveOrganization(id);
    navigate({ kind: "team" });
  };

  return (
    <Show when={isGlobalAdmin()} fallback={<NotTheBackOffice />}>
      <PageTitle
        title="Organizations"
        subtitle="Every tenant in this deployment, not only the ones you belong to."
      />
      <Card class="overflow-hidden">
        <CardHeader
          title="All organizations"
          hint={`${visible().length} shown${hasNext() ? ", more on the next page" : ""}`}
        />
        <div class="flex flex-wrap items-center gap-2 border-b border-line px-5 py-3">
          <div class="w-64">
            <SearchInput
              value={search()}
              placeholder="Search by name or short name…"
              onInput={setSearch}
              onSubmit={() => setApplied(search().trim())}
            />
          </div>
          <select
            class="input h-8 w-40 text-xs"
            aria-label="Class filter"
            value={orgClass()}
            onChange={(event) => setOrgClass(event.currentTarget.value)}
          >
            <option value="">Any class</option>
            <For each={classes()}>{(value) => <option value={value}>{value}</option>}</For>
          </select>
          <Button size="sm" variant="ghost" onClick={refetch} title="Reload">
            Refresh
          </Button>
        </div>

        <Show
          when={visible().length || loading()}
          fallback={
            <div class="px-5 py-4">
              <EmptyState
                title="Nothing matches that"
                description="No organization here has this name and class."
              />
            </div>
          }
        >
          <ul class="divide-y divide-line">
            <For each={visible()}>
              {(organization) => {
                const id = String(organization.id ?? "");
                const current = typeof organization.org_class === "string" ? organization.org_class : "";
                return (
                  <li class="flex flex-wrap items-center gap-3 px-5 py-3">
                    <Avatar name={organizationLabel(organization)} src={avatarOf(organization)} size="sm" />
                    <button
                      type="button"
                      class="min-w-0 flex-1 text-left"
                      onClick={() => navigate({ kind: "record", name: "organization", id })}
                    >
                      <span class="block truncate text-sm font-medium text-ink">
                        {organizationLabel(organization)}
                      </span>
                      <Show when={organization.slug}>
                        <span class="block truncate font-mono text-[0.6875rem] text-faint">
                          {String(organization.slug)}
                        </span>
                      </Show>
                    </button>
                    <Show when={session.organizationId === id}>
                      <Badge tone="accent">Active</Badge>
                    </Show>
                    {/* The class is the column this screen exists to write, so
                        it is edited in the row rather than behind a form: a
                        select, because a typo in a free string is a permission
                        that quietly matches nobody. */}
                    <select
                      class="input h-8 w-36 text-xs"
                      aria-label={`Class of ${organizationLabel(organization)}`}
                      value={current}
                      onChange={(event) => void setClass(id, event.currentTarget.value)}
                    >
                      <option value="">no class</option>
                      <For each={classes()}>{(value) => <option value={value}>{value}</option>}</For>
                    </select>
                    <Show when={session.organizationId !== id}>
                      <Button size="sm" variant="ghost" onClick={() => void setActiveOrganization(id)}>
                        Switch to
                      </Button>
                    </Show>
                    <Button size="sm" variant="ghost" onClick={() => void openTeam(id)}>
                      Team
                    </Button>
                  </li>
                );
              }}
            </For>
          </ul>
        </Show>
        <Pager page={page()} hasNext={hasNext()} onPage={setPage} />
      </Card>
    </Show>
  );
}

// --- users ------------------------------------------------------------------

export function UsersPage() {
  const [search, setSearch] = createSignal("");
  const [applied, setApplied] = createSignal("");
  const [org, setOrg] = createSignal("");
  const [page, setPage] = createSignal(0);

  createEffect(
    () => [applied(), org()] as const,
    () => {
      setPage(0);
    },
  );

  const identity = () => manifest()?.auth.identity_field ?? "email";

  const userLabel = (user: ApiRecord) => {
    const named = user.display_name ?? user.name ?? user[identity()];
    return String(named ?? user.id ?? "Account");
  };

  /** What they sign in with — shown under the name, and what a search matches.
   *  Not `emailOf`, which answers `null` unless Gravatar is switched on: that
   *  one is about fetching a picture, this one is about telling two people
   *  with the same display name apart. */
  const identityValue = (user: ApiRecord) => {
    const value = user[identity()];
    return typeof value === "string" ? value : "";
  };

  /**
   * Every account, or every account in one organisation.
   *
   * The second is a different question and therefore a different endpoint:
   * membership is where "who is in this tenant" is written, so it is asked
   * there and the accounts come back expanded, carrying the role they hold
   * *there*. Searching then happens over the page that was fetched, because a
   * membership row has no name on it to match against.
   */
  const rowsResource = createMemo(async () => {
    const key = { search: applied(), org: org(), page: page(), admin: isGlobalAdmin() };
    if (!key.admin) return [];

    if (key.org) {
      const params = new URLSearchParams({
        organization_id: key.org,
        expand: "user",
        order: "role",
        limit: String(PAGE_SIZE + 1),
        offset: String(key.page * PAGE_SIZE),
      });
      const needle = key.search.toLowerCase();
      return asRecords(await api(`/membership?${params.toString()}`))
        .map((member) => ({ member, user: asRecord(member.user) }))
        .filter((entry): entry is { member: ApiRecord; user: ApiRecord } => Boolean(entry.user))
        .filter(
          ({ user }) =>
            !needle ||
            userLabel(user).toLowerCase().includes(needle) ||
            identityValue(user).toLowerCase().includes(needle),
        )
        .map(({ member, user }) => ({ ...user, __role: member.role ?? null }) as ApiRecord);
    }

    const params = new URLSearchParams({
      limit: String(PAGE_SIZE + 1),
      offset: String(key.page * PAGE_SIZE),
      order: identity(),
    });
    if (key.search) params.set("search", key.search);
    return asRecords(await api(`/user?${params.toString()}`));
  });
  const rows = () => latest(rowsResource) ?? [];
  const loading = () => isPending(rowsResource);
  const refetch = () => refresh(rowsResource);
  const visible = createMemo(() => rows().slice(0, PAGE_SIZE));
  const hasNext = createMemo(() => rows().length > PAGE_SIZE);

  const actAs = async (user: ApiRecord) => {
    try {
      await impersonate(String(user.id ?? ""));
      notify("success", `You are now working as ${userLabel(user)}.`);
      navigate({ kind: "dashboard" });
    } catch (error) {
      reportError(error);
    }
  };

  return (
    <Show when={isGlobalAdmin()} fallback={<NotTheBackOffice />}>
      <PageTitle
        title="Users"
        subtitle="Every account in this deployment, whichever organization it belongs to."
      />
      <Card class="overflow-hidden">
        <CardHeader
          title="All users"
          hint={`${visible().length} shown${hasNext() ? ", more on the next page" : ""}`}
        />
        <div class="flex flex-wrap items-center gap-2 border-b border-line px-5 py-3">
          <div class="w-64">
            <SearchInput
              value={search()}
              placeholder={`Search by ${identity()} or name…`}
              onInput={setSearch}
              onSubmit={() => setApplied(search().trim())}
            />
          </div>
          <select
            class="input h-8 w-52 text-xs"
            aria-label="Organization filter"
            value={org()}
            onChange={(event) => setOrg(event.currentTarget.value)}
          >
            <option value="">Every organization</option>
            <For each={session.organizations}>
              {(organization) => (
                <option value={String(organization.id ?? "")}>{organizationLabel(organization)}</option>
              )}
            </For>
          </select>
          <Button size="sm" variant="ghost" onClick={refetch} title="Reload">
            Refresh
          </Button>
        </div>

        <Show
          when={visible().length || loading()}
          fallback={
            <div class="px-5 py-4">
              <EmptyState title="Nothing matches that" description="No account here matches this search." />
            </div>
          }
        >
          <ul class="divide-y divide-line">
            <For each={visible()}>
              {(user) => {
                const id = String(user.id ?? "");
                const role = typeof user.__role === "string" ? user.__role : "";
                return (
                  <li class="flex flex-wrap items-center gap-3 px-5 py-3">
                    <Avatar name={userLabel(user)} src={avatarOf(user)} email={emailOf(user)} size="sm" />
                    <button
                      type="button"
                      class="min-w-0 flex-1 text-left"
                      onClick={() => navigate({ kind: "record", name: "user", id })}
                    >
                      <span class="block truncate text-sm font-medium text-ink">{userLabel(user)}</span>
                      <Show when={identityValue(user) && identityValue(user) !== userLabel(user)}>
                        <span class="block truncate text-[0.6875rem] text-faint">{identityValue(user)}</span>
                      </Show>
                    </button>
                    <Show when={role}>
                      <Badge tone={role === "admin" ? "accent" : undefined}>{role}</Badge>
                    </Show>
                    <Show when={id === session.userId}>
                      <span class="text-[0.6875rem] text-faint">This is you</span>
                    </Show>
                    <Show when={mayImpersonate(id)}>
                      <Button
                        size="sm"
                        variant="ghost"
                        title={`See the dashboard as ${userLabel(user)} sees it`}
                        onClick={() => void actAs(user)}
                      >
                        Act as
                      </Button>
                    </Show>
                  </li>
                );
              }}
            </For>
          </ul>
        </Show>
        <Pager page={page()} hasNext={hasNext()} onPage={setPage} />
      </Card>
    </Show>
  );
}
