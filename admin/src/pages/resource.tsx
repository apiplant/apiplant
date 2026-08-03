/**
 * The two screens an ordinary resource gets: a table of records, and a form for
 * one of them.
 *
 * The layout is intentionally conventional: a searchable, paginated list, with
 * a row click opening one form holding every field including its relationships.
 * The auth resources are the exception and get purpose-built screens instead;
 * see `settings.tsx`.
 */

import { For, Show, batch, createEffect, createMemo, createResource, createSignal, untrack } from "solid-js";
import { createMutable } from "solid-js/store";
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
  editableFields,
  formatValue,
  ownsRecord,
  readableFields,
  recordLabel,
} from "../fields";
import type { Draft, DraftError } from "../fields";
import { MarkupView } from "../markup";
import {
  api,
  asRecord,
  asRecords,
  can,
  hasRole,
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

function includeOrgContext(resource: ResourceManifest, action: "list" | "read" | "update" | "delete"): boolean {
  if (resource.scope === "organization") return true;
  return Boolean(session.organizationId && hasRole("admin") && resource.permissions[action].value === "owner");
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
  createEffect(() => {
    void props.resource.name;
    void session.userId;
    setFiltersLoaded(false);
    const saved = loadResourceFilter(props.resource.name, {
      query: "",
      ownerOnly: canChooseOwnerFilter(),
      sortField: "",
      sortDescending: false,
    });
    // A saved sort on a column this resource no longer shows would order the
    // table by an invisible value, so it is discarded.
    const savedSort = sortableColumns(props.resource).includes(saved.sortField ?? "")
      ? (saved.sortField as string)
      : "";
    batch(() => {
      setPage(0);
      setSearch(saved.query);
      setApplied(saved.query);
      setOwnerOnly(canChooseOwnerFilter() ? saved.ownerOnly : false);
      setSortField(savedSort);
      setSortDescending(Boolean(saved.sortDescending));
    });
    setFiltersLoaded(true);
  });

  createEffect(() => {
    if (!filtersLoaded()) return;
    saveResourceFilter(props.resource.name, {
      query: applied(),
      ownerOnly: canChooseOwnerFilter() ? ownerOnly() : false,
      sortField: sortField(),
      sortDescending: sortDescending(),
    });
  });

  const needsOrganization = () =>
    props.resource.scope === "organization" && !session.organizationId;

  const [rows, { refetch }] = createResource(
    () => ({
      name: props.resource.name,
      page: page(),
      search: applied(),
      ownerOnly: canChooseOwnerFilter() && ownerOnly(),
      sort: sortField(),
      sortDescending: sortDescending(),
      org: session.organizationId,
      userId: session.userId,
    }),
    async (key) => {
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
      // `?search=` is the API's substring match across every field the model
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
    },
  );

  const visibleRows = createMemo(() => (rows() ?? []).slice(0, PAGE_SIZE));
  const hasNextPage = createMemo(() => (rows() ?? []).length > PAGE_SIZE);

  const columns = createMemo(() =>
    props.resource.columns
      .map((name) => props.resource.fields.find((field) => field.name === name))
      .filter((field): field is FieldManifest => Boolean(field)),
  );

  // Clicking a header cycles ascending → descending → default order, so there
  // is always a way back to "newest first" without a reset control.
  const toggleSort = (field: string) => {
    batch(() => {
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
    });
  };

  const runSearch = () => {
    setPage(0);
    setApplied(search().trim());
  };
  const changeOwnerFilter = (value: string) => {
    setPage(0);
    setOwnerOnly(value === "mine");
  };
  const clearFilters = () => {
    setPage(0);
    setSearch("");
    setApplied("");
    setOwnerOnly(false);
  };

  return (
    <>
      <PageTitle
        title={props.resource.plural}
        subtitle={props.resource.permissions.list.note}
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
            <Button size="sm" variant="ghost" onClick={() => void refetch()} title="Reload">
              Refresh
            </Button>
          </div>
        </CardHeader>

        <Show
          when={!rows.error}
          fallback={
            <div class="px-5 py-6">
              <EmptyState
                title="That list could not be loaded"
                description={rows.error instanceof Error ? rows.error.message : String(rows.error)}
              >
                <Button onClick={() => void refetch()}>Try again</Button>
              </EmptyState>
            </div>
          }
        >
          <Show
            when={rows.loading || visibleRows().length}
            fallback={
              <div class="px-5 py-6">
                <EmptyState
                  title={filtersApplied() ? "Nothing matched those filters" : `No ${props.resource.plural.toLowerCase()} yet`}
                  description={
                    filtersApplied()
                      ? "Try a different search, or clear the filters to see everything."
                      : can(props.resource, "create")
                        ? `Create the first one to get started.`
                        : "There is nothing here for you to see yet."
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
                    when={!rows.loading}
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
        </Show>
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

export function RecordPage(props: { resource: ResourceManifest; id: string | null }) {
  const isNew = () => props.id === null;
  const [saving, setSaving] = createSignal(false);
  const [deleting, setDeleting] = createSignal(false);
  const [confirmDelete, setConfirmDelete] = createSignal(false);
  const [errors, setErrors] = createSignal<DraftError[]>([]);
  const draft = createMutable<Draft>({});

  const [record, { refetch }] = createResource(
    () => ({ name: props.resource.name, id: props.id, org: session.organizationId }),
    async (key) => {
      if (!key.id) return null;
      const resource = resourceByName(key.name);
      if (!resource) return null;
      const expand = expandParam(resource);
      const query = expand ? `?expand=${encodeURIComponent(expand)}` : "";
      return asRecord(
        await api(`/${resource.name}/${encodeURIComponent(key.id)}${query}`, {
          org: includeOrgContext(resource, "read"),
        }),
      );
    },
  );

  // Reset the form whenever the record or the resource changes underneath it.
  //
  // The rewrite must be untracked: reading the draft's own keys in order to
  // clear them would make this effect depend on the store it writes, causing it
  // to re-run indefinitely.
  createEffect(() => {
    const loaded = (isNew() ? null : record()) ?? null;
    if (!isNew() && record.loading) return;
    const fresh = createDraft(props.resource, loaded);
    untrack(() => {
      for (const key of Object.keys(draft)) delete draft[key];
      Object.assign(draft, fresh);
    });
    setErrors([]);
  });

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
    const { payload, errors: problems } = buildPayload(props.resource, draft);
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
          org: isNew()
            ? props.resource.scope === "organization"
            : includeOrgContext(props.resource, "update"),
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

      <Show
        when={isNew() || !record.loading}
        fallback={
          <div class="flex items-center gap-2 px-1 py-10 text-sm text-faint">
            <Spinner /> Loading…
          </div>
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

  const [rows] = createResource(
    () => ({
      parent: props.parentId,
      parentName: parentName(),
      child: props.child.resource,
      org: session.organizationId,
    }),
    async (key) => {
      const child = resourceByName(key.child);
      if (!child || !key.parentName) return [];
      // `via` selects which reference to follow, which matters when the child
      // references the same parent more than once, as with billing and shipping
      // addresses.
      const params = new URLSearchParams({ limit: "10", via: props.child.field });
      const expand = expandParam(child);
      if (expand) params.set("expand", expand);
      return asRecords(
        await api(
          `/${key.parentName}/${encodeURIComponent(key.parent)}/${child.name}?${params.toString()}`,
          { org: child.scope === "organization" },
        ),
      );
    },
  );

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
    <Show when={resource()}>
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
            when={!rows.loading}
            fallback={<p class="px-5 py-4 text-xs text-faint">Loading…</p>}
          >
            <Show
              when={(rows() ?? []).length}
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
        </Card>
      )}
    </Show>
  );
}
