/**
 * The two screens an ordinary resource gets: a table of records, and a form for
 * one of them.
 *
 * The layout is intentionally conventional: a searchable, paginated list, with
 * a row click opening one form holding every field including its relationships.
 * The auth resources are the exception and get purpose-built screens instead;
 * see `settings.tsx`.
 */

import { Errored, For, Show, createEffect, createMemo, createSignal, isPending, latest, refresh } from "solid-js";
import { loadResourceFilter, saveResourceFilter } from "../filters";
import {
  Button,
  Card,
  CardHeader,
  ConfirmDialog,
  EmptyState,
  PageTitle,
  SearchInput,
  SkeletonRows,
  Spinner,
} from "../ui";
import {
  FieldEditor,
  buildPayload,
  createDraft,
  createDraftStore,
  editableFields,
  formatValue,
  ownsRecord,
  readableFields,
  recordLabel,
} from "../fields";
import type { ApiError } from "../store";
import type { DraftError } from "../fields";
import { MarkupView } from "../markup";
import {
  api,
  asRecord,
  asRecords,
  can,
  hasRole,
  impersonate,
  includeOrgContext,
  mayImpersonate,
  navigate,
  notify,
  reportError,
  resourceByName,
  session,
} from "../store";
import type { ApiRecord, ChildManifest, FieldManifest, ResourceManifest } from "../types";

const PAGE_SIZE = 25;

/** Ask the API to inline the records a row points at, so the table can show
 *  "Acme Ltd" where the column holds a uuid. */
function expandParam(resource: ResourceManifest): string {
  return resource.relations.map((relation) => relation.relation).join(",");
}

// --- list ------------------------------------------------------------------

export function ResourceListPage(props: { resource: ResourceManifest }) {
  const [page, setPage] = createSignal(0);
  const [search, setSearch] = createSignal("");
  const [applied, setApplied] = createSignal("");
  const [ownerOnly, setOwnerOnly] = createSignal(false);
  const [filtersLoaded, setFiltersLoaded] = createSignal(false);
  // "" means the resource's default order, which is newest first.
  const [sortField, setSortField] = createSignal("");
  const [sortDescending, setSortDescending] = createSignal(false);

  const supportsOwnerFilter = createMemo(() => Boolean(session.userId && props.resource.owner_field.trim()));
  const canChooseOwnerFilter = createMemo(() => hasRole("admin") && supportsOwnerFilter());
  const filtersApplied = createMemo(() => Boolean(applied() || ownerOnly()));

  // Changing resource must not inherit the previous one's page or search.
  createEffect(
    () => [props.resource, session.userId, canChooseOwnerFilter()] as const,
    ([resource, , canChooseOwner]) => {
      setFiltersLoaded(false);
      const saved = loadResourceFilter(resource.name, {
        query: "",
        ownerOnly: canChooseOwner,
        sortField: "",
        sortDescending: false,
      });
      // A saved sort on a column this resource no longer shows would order the
      // table by an invisible value, so it is discarded.
      const savedSort = sortableColumns(resource).includes(saved.sortField ?? "")
        ? (saved.sortField as string)
        : "";
      setPage(0);
      setSearch(saved.query);
      setApplied(saved.query);
      setOwnerOnly(canChooseOwner ? saved.ownerOnly : false);
      setSortField(savedSort);
      setSortDescending(Boolean(saved.sortDescending));
      setFiltersLoaded(true);
    },
  );

  createEffect(
    () =>
      [
        filtersLoaded(),
        props.resource.name,
        applied(),
        canChooseOwnerFilter(),
        ownerOnly(),
        sortField(),
        sortDescending(),
      ] as const,
    ([loaded, name, query, canChooseOwner, owner, field, descending]) => {
      if (!loaded) return;
      saveResourceFilter(name, {
        query,
        ownerOnly: canChooseOwner ? owner : false,
        sortField: field,
        sortDescending: descending,
      });
    },
  );

  const needsOrganization = () =>
    props.resource.scope === "organization" && !session.organizationId;

  const rowsResource = createMemo(async () => {
      const key = {
        name: props.resource.name,
        page: page(),
        search: applied(),
        ownerOnly: canChooseOwnerFilter() && ownerOnly(),
        sort: sortField(),
        sortDescending: sortDescending(),
        org: session.organizationId,
        userId: session.userId,
      };
      const resource = resourceByName(key.name);
      if (!resource) return [];
      if (resource.scope === "organization" && !key.org) return [];

      const params = new URLSearchParams();
      // Fetching one extra row is the cheapest way to detect a next page,
      // since the list endpoint returns no total.
      params.set("limit", String(PAGE_SIZE + 1));
      params.set("offset", String(key.page * PAGE_SIZE));
      const expand = expandParam(resource);
      if (expand) params.set("expand", expand);
      // `?search=` is the API's substring match across every field the resource
      // names. Matching only whole values would make this a filter rather than
      // a search.
      if (key.search && searchable(resource)) params.set("search", key.search);
      // Left unset, the API orders newest first, which is the desired default.
      if (key.sort) params.set("order", key.sortDescending ? `-${key.sort}` : key.sort);
      if (key.ownerOnly && resource.owner_field && key.userId) params.set(resource.owner_field, key.userId);

      return asRecords(
        await api(`/${resource.name}?${params.toString()}`, {
          org: includeOrgContext(resource, "list"),
        }),
      );
  });

  // Read through `latest` so the table keeps the rows it already has while the
  // next page loads; `isPending` drives the skeleton, as `rows.loading` did.
  const rows = () => latest(rowsResource) ?? [];
  const rowsLoading = () => isPending(rowsResource);
  const refetch = () => refresh(rowsResource);

  const visibleRows = createMemo(() => rows().slice(0, PAGE_SIZE));
  const hasNextPage = createMemo(() => rows().length > PAGE_SIZE);

  const columns = createMemo(() =>
    props.resource.columns
      .map((name) => props.resource.fields.find((field) => field.name === name))
      .filter((field): field is FieldManifest => Boolean(field)),
  );

  // Clicking a header cycles ascending → descending → default order, so there
  // is always a way back to "newest first" without a reset control.
  const toggleSort = (field: string) => {
    setPage(0);
    if (sortField() !== field) {
      setSortField(field);
      setSortDescending(false);
    } else if (!sortDescending()) {
      setSortDescending(true);
    } else {
      setSortField("");
      setSortDescending(false);
    }
  };

  const runSearch = () => {
    setPage(0);
    setApplied(search().trim());
  };
  const changeOwnerFilter = (value: string) => {
    setPage(0);
    setOwnerOnly(value === "mine");
  };
  // The `list` note explains who may see this screen, which is worth saying
  // when it is a limit on *you* ("Limited to records you created.") and noise
  // when it is a role you already hold — you are looking at the list, so being
  // told you need the role to look at it explains nothing.
  const subtitle = createMemo(() => {
    const policy = props.resource.permissions.list;
    if (policy.role && hasRole(policy.role)) return undefined;
    return policy.note;
  });

  const clearFilters = () => {
    setPage(0);
    setSearch("");
    setApplied("");
    setOwnerOnly(false);
  };

  /** Borrow the account on this row and land on the dashboard as them. */
  const actAsRow = async (row: ApiRecord) => {
    try {
      await impersonate(String(row.id ?? ""));
      notify("success", `You are now working as ${recordLabel(props.resource, row)}.`);
      navigate({ kind: "dashboard" });
    } catch (error) {
      reportError(error);
    }
  };

  return (
    <>
      <PageTitle
        title={props.resource.plural}
        subtitle={subtitle()}
      >
        <Show when={can(props.resource, "create")}>
          <Button variant="primary" onClick={() => navigate({ kind: "new", name: props.resource.name })}>
            New {props.resource.label.toLowerCase()}
          </Button>
        </Show>
      </PageTitle>

      <Show when={needsOrganization()}>
        <Card class="mb-4 border-warn-line bg-warn-soft/40">
          <p class="px-4 py-3 text-[0.8125rem] text-ink">
            Choose an organization from the top bar to see {props.resource.plural.toLowerCase()}.
          </p>
        </Card>
      </Show>

      <Card class="overflow-hidden">
        <CardHeader title={props.resource.plural} hint={countHint(visibleRows().length, page(), hasNextPage())}>
          <div class="flex flex-wrap items-center justify-end gap-2">
            <Show when={searchable(props.resource)}>
              <div class="w-56">
                <SearchInput
                  value={search()}
                  placeholder={`Search by ${searchLabel(props.resource)}…`}
                  onInput={setSearch}
                  onSubmit={runSearch}
                />
              </div>
            </Show>
            <Show when={canChooseOwnerFilter()}>
              <select
                class="input w-36"
                aria-label="Ownership filter"
                value={ownerOnly() ? "mine" : "everybody"}
                onChange={(event) => changeOwnerFilter(event.currentTarget.value)}
              >
                <option value="everybody">Everybody</option>
                <option value="mine">Only mine</option>
              </select>
            </Show>
            <Button size="sm" variant="ghost" onClick={refetch} title="Reload">
              Refresh
            </Button>
          </div>
        </CardHeader>

        <Errored
          fallback={(error, reset) => (
            <div class="px-5 py-6">
              <EmptyState
                title="That list could not be loaded"
                description={error() instanceof Error ? (error() as Error).message : String(error())}
              >
                <Button
                  onClick={() => {
                    refetch();
                    reset();
                  }}
                >
                  Try again
                </Button>
              </EmptyState>
            </div>
          )}
        >
          <Show
            when={rowsLoading() || visibleRows().length}
            fallback={
              <div class="px-5 py-6">
                <EmptyState
                  title={filtersApplied() ? "Nothing matched those filters" : `No ${props.resource.plural.toLowerCase()} yet`}
                  description={
                    filtersApplied()
                      ? "Try a different search, or clear the filters to see everything."
                      : can(props.resource, "create")
                        ? `Create the first one to get started.`
                        : "Nothing to show yet."
                  }
                >
                  <Show when={filtersApplied()}>
                    <Button
                      onClick={clearFilters}
                    >
                      Clear filters
                    </Button>
                  </Show>
                  <Show when={!applied() && can(props.resource, "create")}>
                    <Button
                      variant="primary"
                      onClick={() => navigate({ kind: "new", name: props.resource.name })}
                    >
                      New {props.resource.label.toLowerCase()}
                    </Button>
                  </Show>
                </EmptyState>
              </div>
            }
          >
            <div class="overflow-x-auto">
              <table class="min-w-full text-sm">
                <thead class="border-b border-line bg-surface-2/50 text-left text-[0.6875rem] uppercase tracking-[0.08em] text-faint">
                  <tr>
                    <For each={columns()}>
                      {(field) => (
                        <SortableHeader
                          field={field}
                          sortable={sortableColumns(props.resource).includes(field.name)}
                          direction={sortField() === field.name ? (sortDescending() ? "desc" : "asc") : null}
                          onSort={() => toggleSort(field.name)}
                        />
                      )}
                    </For>
                    <th class="w-10 px-4 py-2.5" />
                  </tr>
                </thead>
                <tbody class="divide-y divide-line">
                  <Show
                    when={!rowsLoading()}
                    fallback={<SkeletonRows columns={columns().length + 1} />}
                  >
                    <For each={visibleRows()}>
                      {(row) => (
                        <tr
                          class="cursor-pointer transition-colors hover:bg-surface-2/60"
                          onClick={() =>
                            navigate({
                              kind: "record",
                              name: props.resource.name,
                              id: String(row.id ?? ""),
                            })
                          }
                        >
                          <For each={columns()}>
                            {(field, index) => (
                              <td
                                class={`px-4 py-3 align-middle ${
                                  index() === 0 ? "font-medium text-ink" : "text-muted"
                                }`}
                              >
                                {formatValue(field, row)}
                              </td>
                            )}
                          </For>
                          <td class="px-4 py-3 text-right text-faint">
                            {/*
                              Where the back office finds somebody it shares no
                              organisation with. The team screen offers this per
                              member of one organisation, which is everybody an
                              org admin may borrow — but a global admin may
                              borrow anyone, and this list is the only screen
                              that holds them all. `stopPropagation` because the
                              row itself opens the record.
                            */}
                            <Show when={props.resource.name === "user" && mayImpersonate(String(row.id ?? ""))}>
                              <button
                                type="button"
                                class="mr-3 rounded-md border border-line px-2 py-1 text-[0.6875rem] text-muted transition-colors hover:border-accent hover:text-ink"
                                title="See the dashboard as this person sees it"
                                onClick={(event) => {
                                  event.stopPropagation();
                                  void actAsRow(row);
                                }}
                              >
                                Act as
                              </button>
                            </Show>
                            <svg
                              class="inline h-3.5 w-3.5"
                              viewBox="0 0 16 16"
                              fill="none"
                              stroke="currentColor"
                              stroke-width="1.5"
                            >
                              <path d="m6 3.5 4.5 4.5L6 12.5" stroke-linecap="round" stroke-linejoin="round" />
                            </svg>
                          </td>
                        </tr>
                      )}
                    </For>
                  </Show>
                </tbody>
              </table>
            </div>

            <Show when={page() > 0 || hasNextPage()}>
              <div class="flex items-center justify-between gap-3 border-t border-line px-4 py-3">
                <p class="text-xs text-faint">Page {page() + 1}</p>
                <div class="flex gap-2">
                  <Button size="sm" disabled={page() === 0} onClick={() => setPage((value) => value - 1)}>
                    Previous
                  </Button>
                  <Button size="sm" disabled={!hasNextPage()} onClick={() => setPage((value) => value + 1)}>
                    Next
                  </Button>
                </div>
              </div>
            </Show>
          </Show>
        </Errored>
      </Card>
    </>
  );
}

function countHint(shown: number, page: number, hasNext: boolean): string {
  if (!shown) return "";
  const first = page * PAGE_SIZE + 1;
  return `Showing ${first}–${first + shown - 1}${hasNext ? "" : " (end of list)"}`;
}

/** Whether the resource has anything for the search box to look in. */
function searchable(resource: ResourceManifest): boolean {
  return (resource.search_fields ?? []).length > 0 || Boolean(resource.search_field);
}

function searchLabel(resource: ResourceManifest): string {
  const names = resource.search_fields?.length
    ? resource.search_fields
    : resource.search_field
      ? [resource.search_field]
      : [];
  const labels = names.map((name) => {
    const field = resource.fields.find((entry) => entry.name === name);
    return (field?.label ?? name).toLowerCase();
  });
  if (!labels.length) return "name";
  // Up to three columns are listed; beyond that a count is clearer.
  if (labels.length > 3) return `${labels.length} fields`;
  if (labels.length === 1) return labels[0];
  return `${labels.slice(0, -1).join(", ")} or ${labels[labels.length - 1]}`;
}

/** Columns the API will accept as an `?order=` key.
 *
 *  `json` is excluded: Postgres can order by it, but the result sorts the
 *  serialised text, which is not what clicking that header implies. */
function sortableColumns(resource: ResourceManifest): string[] {
  return resource.columns.filter((name) => {
    const field = resource.fields.find((entry) => entry.name === name);
    return Boolean(field) && field!.type !== "json" && !field!.hidden;
  });
}

/** A table header that sorts, and shows which way it is sorting. */
function SortableHeader(props: {
  field: FieldManifest;
  sortable: boolean;
  direction: "asc" | "desc" | null;
  onSort: () => void;
}) {
  return (
    <th
      class="px-4 py-2.5 font-semibold"
      aria-sort={props.direction === "asc" ? "ascending" : props.direction === "desc" ? "descending" : "none"}
    >
      <Show when={props.sortable} fallback={props.field.label}>
        <button
          type="button"
          class={`group inline-flex items-center gap-1 uppercase tracking-[0.08em] transition-colors hover:text-ink ${
            props.direction ? "text-ink" : ""
          }`}
          title={`Sort by ${props.field.label.toLowerCase()}`}
          onClick={props.onSort}
        >
          {props.field.label}
          <SortArrow direction={props.direction} />
        </button>
      </Show>
    </th>
  );
}

/** The caret beside a sortable header: faint until the column is the sort. */
function SortArrow(props: { direction: "asc" | "desc" | null }) {
  return (
    <svg
      class={`h-3 w-3 shrink-0 transition-opacity ${
        props.direction ? "opacity-100" : "opacity-0 group-hover:opacity-40"
      }`}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="1.75"
      aria-hidden="true"
    >
      <path
        d={props.direction === "desc" ? "M4 6.5 8 10.5l4-4" : "M4 9.5 8 5.5l4 4"}
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

// --- record ----------------------------------------------------------------

/** What one record read came back with: the row, or the status that refused it. */
type LoadedRecord = { record: ApiRecord | null; failed?: number; message?: string };

/** An error's own words, since a bare status is not a sentence. */
function messageOf(failure: unknown): string {
  return failure instanceof Error ? failure.message : String(failure);
}

export function RecordPage(props: { resource: ResourceManifest; id: string | null }) {
  const isNew = () => props.id === null;
  const [saving, setSaving] = createSignal(false);
  const [deleting, setDeleting] = createSignal(false);
  const [confirmDelete, setConfirmDelete] = createSignal(false);
  const [errors, setErrors] = createSignal<DraftError[]>([]);
  const draft = createDraftStore();

  /**
   * The record, or the status that refused it.
   *
   * Tagged rather than thrown, and for the same reason the related lists are:
   * the loaded row is read all over this screen — the heading above, the
   * permission memos, the effect that fills the form — and several of those
   * reads happen outside any boundary. An async read that rejects with nobody
   * to catch it does not just blank the screen: it halts the application's
   * reactivity, and the whole dashboard stops answering navigation. So the
   * failure is a value, and every reader sees an empty record instead.
   */
  const recordResource = createMemo(async (): Promise<LoadedRecord> => {
    const key = { name: props.resource.name, id: props.id, org: session.organizationId };
    if (!key.id) return { record: null };
    const resource = resourceByName(key.name);
    if (!resource) return { record: null };
    const expand = expandParam(resource);
    const query = expand ? `?expand=${encodeURIComponent(expand)}` : "";
    try {
      return {
        record: asRecord(
          await api(`/${resource.name}/${encodeURIComponent(key.id)}${query}`, {
            org: includeOrgContext(resource, "read"),
          }),
        ),
      };
    } catch (failure) {
      return { record: null, failed: (failure as ApiError).status ?? 0, message: messageOf(failure) };
    }
  });

  const record = () => latest(recordResource)?.record ?? null;
  const recordLoading = () => isPending(recordResource);
  const refetch = () => refresh(recordResource);
  /** Set when the read was refused for a reason worth naming — anything but the
   *  plain "it is not there", which the empty state below already says better. */
  const loadFailure = () => {
    const loaded = latest(recordResource);
    if (!loaded?.failed || loaded.failed === 404) return null;
    return loaded.message ?? "That did not work.";
  };

  // Reset the form whenever the record or the resource changes underneath it.
  // The apply phase is untracked, so writing the draft cannot feed back into
  // the compute that decided to write it.
  createEffect(
    () => [props.resource, isNew() ? null : record(), recordLoading()] as const,
    ([resource, loaded, loading]) => {
      if (!isNew() && loading) return;
      draft.reset(createDraft(resource, loaded ?? null));
      setErrors([]);
    },
  );

  const errorFor = (name: string) => errors().find((entry) => entry.field === name)?.message ?? null;

  const mayEdit = createMemo(() => {
    if (isNew()) return can(props.resource, "create");
    const policy = props.resource.permissions.update;
    // An `owner` policy is the one case the manifest alone cannot resolve, so
    // check the loaded row.
    if (policy.value === "owner") return ownsRecord(props.resource, record() ?? null);
    return can(props.resource, "update");
  });

  const mayDelete = createMemo(() => {
    if (isNew()) return false;
    const policy = props.resource.permissions.delete;
    if (policy.value === "owner") return ownsRecord(props.resource, record() ?? null);
    return can(props.resource, "delete");
  });

  const save = async () => {
    const { payload, errors: problems } = buildPayload(props.resource, draft.values);
    setErrors(problems);
    if (problems.length) {
      notify("error", problems.length === 1 ? problems[0].message : "Please fix the highlighted fields.");
      return;
    }
    setSaving(true);
    try {
      const saved = asRecord(
        await api(
          isNew()
            ? `/${props.resource.name}`
            : `/${props.resource.name}/${encodeURIComponent(props.id!)}`,
          {
            method: isNew() ? "POST" : "PATCH",
            body: payload,
          org: includeOrgContext(props.resource, isNew() ? "create" : "update"),
        },
        ),
      );
      notify("success", isNew() ? `${props.resource.label} created.` : "Changes saved.");
      if (isNew() && saved?.id) {
        navigate({ kind: "record", name: props.resource.name, id: String(saved.id) });
      } else {
        void refetch();
      }
    } catch (failure) {
      reportError(failure);
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    setDeleting(true);
    try {
      await api(`/${props.resource.name}/${encodeURIComponent(props.id!)}`, {
        method: "DELETE",
        org: includeOrgContext(props.resource, "delete"),
      });
      notify("success", `${props.resource.label} deleted.`);
      navigate({ kind: "resource", name: props.resource.name });
    } catch (failure) {
      reportError(failure);
    } finally {
      setDeleting(false);
      setConfirmDelete(false);
    }
  };

  const heading = () =>
    isNew() ? `New ${props.resource.label.toLowerCase()}` : recordLabel(props.resource, record() ?? null);

  /** Borrow this account and land on the dashboard as them. */
  const actAs = async () => {
    try {
      await impersonate(props.id ?? "");
      notify("success", `You are now working as ${heading()}.`);
      navigate({ kind: "dashboard" });
    } catch (error) {
      reportError(error);
    }
  };

  return (
    <>
      <button
        type="button"
        class="mb-3 inline-flex items-center gap-1.5 text-xs text-faint transition-colors hover:text-ink"
        onClick={() => navigate({ kind: "resource", name: props.resource.name })}
      >
        <svg class="h-3 w-3" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6">
          <path d="M10 3.5 5.5 8l4.5 4.5" stroke-linecap="round" stroke-linejoin="round" />
        </svg>
        All {props.resource.plural.toLowerCase()}
      </button>

      <PageTitle title={heading()} subtitle={isNew() ? undefined : props.resource.label}>
        {/*
          The other door into impersonation. The team screen offers it per row
          for the people an organisation's admin administers; this one is where
          a global admin finds somebody they share no organisation with — the
          list beside it offers the same thing without opening the record.
        */}
        <Show when={props.resource.name === "user" && !isNew() && mayImpersonate(props.id ?? "")}>
          <Button
            onClick={() => void actAs()}
            title="See the dashboard as this person sees it"
          >
            Act as this user
          </Button>
        </Show>
        <Show when={mayDelete()}>
          <Button variant="danger" onClick={() => setConfirmDelete(true)}>
            Delete
          </Button>
        </Show>
        <Show when={mayEdit()}>
          <Button variant="primary" loading={saving()} onClick={() => void save()}>
            {isNew() ? `Create ${props.resource.label.toLowerCase()}` : "Save changes"}
          </Button>
        </Show>
      </PageTitle>

      {/*
        The boundary is the second line of defence: the read itself no longer
        rejects, so what reaches here is whatever else this screen's children
        might throw — and none of it should be able to halt the dashboard.
      */}
      <Errored
        fallback={(error, reset) => (
          <EmptyState title="That record could not be loaded" description={messageOf(error())}>
            <Button
              onClick={() => {
                refetch();
                reset();
              }}
            >
              Try again
            </Button>
          </EmptyState>
        )}
      >
      <Show
        when={isNew() || !recordLoading()}
        fallback={
          <div class="flex items-center gap-2 px-1 py-10 text-sm text-faint">
            <Spinner /> Loading…
          </div>
        }
      >
        <Show
          when={!loadFailure()}
          fallback={
            <EmptyState title="That record could not be loaded" description={loadFailure() ?? ""}>
              <Button onClick={refetch}>Try again</Button>
            </EmptyState>
          }
        >
        <Show
          when={isNew() || record()}
          fallback={
            <EmptyState
              title="That record is not here"
              description="It may have been deleted, or it belongs to a different organization."
            >
              <Button onClick={() => navigate({ kind: "resource", name: props.resource.name })}>
                Back to {props.resource.plural.toLowerCase()}
              </Button>
            </EmptyState>
          }
        >
          <div class="grid gap-4 xl:grid-cols-[minmax(0,1fr)_20rem]">
            <div class="space-y-4">
              <Card>
                <CardHeader title="Details" />
                <div data-ai-assist-scope class="grid gap-4 px-5 py-5 sm:grid-cols-2">
                  <Show
                    when={editableFields(props.resource).length}
                    fallback={
                      <p class="text-xs text-faint sm:col-span-2">
                        This record has no fields you can edit.
                      </p>
                    }
                  >
                    <For each={editableFields(props.resource)}>
                      {(field) => (
                        <div class={field.widget === "textarea" || field.widget === "json" ? "sm:col-span-2" : ""}>
                          <FieldEditor
                            field={field}
                            draft={draft}
                            error={errorFor(field.name)}
                            disabled={!mayEdit()}
                          />
                        </div>
                      )}
                    </For>
                  </Show>
                </div>
              </Card>

              <Show when={!isNew()}>
                <For each={props.resource.children}>
                  {(child) => <RelatedList parentId={props.id!} child={child} />}
                </For>
              </Show>
            </div>

            <div class="space-y-4">
              <Show when={!isNew() && record()}>
                {(loaded) => <RecordSummary resource={props.resource} record={loaded()} />}
              </Show>
              <Show when={!mayEdit() && !isNew()}>
                <Card class="border-warn-line bg-warn-soft/30">
                  <p class="px-4 py-3 text-[0.8125rem] leading-relaxed text-ink">
                    You can view this {props.resource.label.toLowerCase()} but not change it.{" "}
                    {props.resource.permissions.update.note}
                  </p>
                </Card>
              </Show>
            </div>
          </div>
        </Show>
        </Show>
      </Show>
      </Errored>

      <ConfirmDialog
        open={confirmDelete()}
        title={`Delete this ${props.resource.label.toLowerCase()}?`}
        description={`“${heading()}” will be removed permanently. Anything that depends on it may stop working.`}
        confirmLabel="Delete"
        danger
        busy={deleting()}
        onConfirm={() => void remove()}
        onCancel={() => setConfirmDelete(false)}
      />
    </>
  );
}

/** The read-only facts about a record: when it was made, what it links to. */
function RecordSummary(props: { resource: ResourceManifest; record: ApiRecord }) {
  const readOnly = createMemo(() =>
    readableFields(props.resource).filter((field) => !field.writable),
  );
  return (
    <Card>
      <CardHeader title="About" />
      <dl class="space-y-3 px-5 py-4 text-[0.8125rem]">
        <For each={props.resource.relations}>
          {(relation) => {
            const related = () => asRecord(props.record[relation.relation]);
            return (
              <Show when={props.record[relation.field]}>
                <div>
                  <dt class="text-[0.6875rem] uppercase tracking-wide text-faint">{relation.label}</dt>
                  <dd class="mt-0.5">
                    <button
                      type="button"
                      class="text-accent transition-opacity hover:underline"
                      onClick={() =>
                        navigate({
                          kind: "record",
                          name: relation.target,
                          id: String(props.record[relation.field] ?? ""),
                        })
                      }
                    >
                      {recordLabel(resourceByName(relation.target), related())}
                    </button>
                  </dd>
                </div>
              </Show>
            );
          }}
        </For>

        <For each={readOnly()}>
          {(field) => (
            <div>
              <dt class="text-[0.6875rem] uppercase tracking-wide text-faint">{field.label}</dt>
              <dd class="mt-0.5 text-muted">
                <Show
                  when={field.format !== "plain" && typeof props.record[field.name] === "string"}
                  fallback={formatValue(field, props.record)}
                >
                  <MarkupView value={String(props.record[field.name])} format={field.format} />
                </Show>
              </dd>
            </div>
          )}
        </For>

        <For each={["created_at", "updated_at"] as const}>
          {(key) => (
            <Show when={typeof props.record[key] === "string"}>
              <div>
                <dt class="text-[0.6875rem] uppercase tracking-wide text-faint">
                  {key === "created_at" ? "Created" : "Last updated"}
                </dt>
                <dd class="mt-0.5 text-muted">
                  {new Date(String(props.record[key])).toLocaleString(undefined, {
                    dateStyle: "medium",
                    timeStyle: "short",
                  })}
                </dd>
              </div>
            </Show>
          )}
        </For>
      </dl>
    </Card>
  );
}

/** What one related list came back with: its rows, or the status that refused them. */
type RelatedRows = { rows: ApiRecord[]; failed?: number };

/**
 * Records that reference this one, such as an order's lines or a customer's
 * orders. Shown inline, since related records are a primary reason to open a
 * record screen.
 */
function RelatedList(props: { parentId: string; child: ChildManifest }) {
  const resource = createMemo(() => resourceByName(props.child.resource));
  /** The resource on the other end of the child's reference — the URL's first
   *  segment for the nested collection endpoint. */
  const parentName = createMemo(
    () =>
      resource()?.relations.find((relation) => relation.field === props.child.field)?.target ?? null,
  );

  /**
   * The rows, or why there are none to show.
   *
   * A record lists every child the manifest names, and not all of them answer
   * for every caller: a nested collection whose child is `private` is not
   * mounted at all (404), and one the session may not list is refused (403).
   * Neither is an error the operator can act on, and neither may be allowed to
   * reject: this runs on a record screen that draws several of these at once,
   * so one unreadable collection would otherwise take the whole screen — and,
   * unhandled, the application's reactivity with it.
   */
  const rowsResource = createMemo(async (): Promise<RelatedRows> => {
      const key = {
        parent: props.parentId,
        parentName: parentName(),
        child: props.child.resource,
        org: session.organizationId,
      };
      const child = resourceByName(key.child);
      if (!child || !key.parentName) return { rows: [] };
      // `via` selects which reference to follow, which matters when the child
      // references the same parent more than once, as with billing and shipping
      // addresses.
      const params = new URLSearchParams({ limit: "10", via: props.child.field });
      const expand = expandParam(child);
      if (expand) params.set("expand", expand);
      try {
        return {
          rows: asRecords(
            await api(
              `/${key.parentName}/${encodeURIComponent(key.parent)}/${child.name}?${params.toString()}`,
              // The nested endpoint authorizes the *child*'s list policy, so the
              // header it needs is the child's, not the parent's.
              { org: includeOrgContext(child, "list") },
            ),
          ),
        };
      } catch (failure) {
        return { rows: [], failed: (failure as ApiError).status ?? 0 };
      }
  });

  const rows = () => latest(rowsResource)?.rows ?? [];
  const rowsLoading = () => isPending(rowsResource);
  /** A collection this caller cannot read here is not shown at all: an empty
   *  card would claim there are none, which is a different statement. */
  const hidden = () => {
    const status = latest(rowsResource)?.failed;
    return status === 403 || status === 404;
  };
  const failed = () => Boolean(latest(rowsResource)?.failed) && !hidden();

  const columns = createMemo(() => {
    const child = resource();
    if (!child) return [];
    return child.columns
      .filter((name) => name !== props.child.field)
      .map((name) => child.fields.find((field) => field.name === name))
      .filter((field): field is FieldManifest => Boolean(field))
      .slice(0, 3);
  });

  return (
    <Show when={hidden() ? null : resource()}>
      {(child) => (
        <Card class="overflow-hidden">
          <CardHeader title={props.child.label}>
            <Show when={can(child(), "create")}>
              <Button size="sm" onClick={() => navigate({ kind: "new", name: child().name })}>
                Add
              </Button>
            </Show>
          </CardHeader>
          <Show
            when={!rowsLoading()}
            fallback={<p class="px-5 py-4 text-xs text-faint">Loading…</p>}
          >
            <Show
              when={!failed()}
              fallback={
                <div class="flex flex-wrap items-center gap-3 px-5 py-4">
                  <p class="text-xs text-faint">These could not be loaded.</p>
                  <Button size="sm" variant="ghost" onClick={() => refresh(rowsResource)}>
                    Try again
                  </Button>
                </div>
              }
            >
            <Show
              when={rows().length}
              fallback={
                <p class="px-5 py-4 text-xs text-faint">
                  No {props.child.label.toLowerCase()} yet.
                </p>
              }
            >
              <table class="min-w-full text-sm">
                <tbody class="divide-y divide-line">
                  <For each={rows()}>
                    {(row) => (
                      <tr
                        class="cursor-pointer transition-colors hover:bg-surface-2/60"
                        onClick={() =>
                          navigate({ kind: "record", name: child().name, id: String(row.id ?? "") })
                        }
                      >
                        <td class="px-5 py-2.5 font-medium text-ink">{recordLabel(child(), row)}</td>
                        <For each={columns()}>
                          {(field) => <td class="px-4 py-2.5 text-muted">{formatValue(field, row)}</td>}
                        </For>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
              <Show when={(rows() ?? []).length >= 10}>
                <div class="border-t border-line px-5 py-2.5">
                  <Button size="sm" variant="ghost" onClick={() => navigate({ kind: "resource", name: child().name })}>
                    See all {child().plural.toLowerCase()}
                  </Button>
                </div>
              </Show>
            </Show>
            </Show>
          </Show>
        </Card>
      )}
    </Show>
  );
}
