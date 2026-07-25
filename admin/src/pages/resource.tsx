/**
 * The two screens an ordinary resource gets: a table of records, and a form for
 * one of them.
 *
 * This is the *traditional* shape on purpose — a searchable, paginated list,
 * click a row to edit, one form holding every field including its
 * relationships. It is what people already know. The auth resources are the
 * exception and get purpose-built screens instead; see `settings.tsx`.
 */

import { For, Show, createEffect, createMemo, createResource, createSignal, untrack } from "solid-js";
import { createMutable } from "solid-js/store";
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
import {
  api,
  asRecord,
  asRecords,
  can,
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

  // A change of resource must not inherit the previous one's page or search.
  createEffect(() => {
    void props.resource.name;
    setPage(0);
    setSearch("");
    setApplied("");
  });

  const needsOrganization = () =>
    props.resource.scope === "organization" && !session.organizationId;

  const [rows, { refetch }] = createResource(
    () => ({
      name: props.resource.name,
      page: page(),
      search: applied(),
      org: session.organizationId,
    }),
    async (key) => {
      const resource = resourceByName(key.name);
      if (!resource) return [];
      if (resource.scope === "organization" && !key.org) return [];

      const params = new URLSearchParams();
      // One extra row is the cheapest way to know whether a "next" page
      // exists — the list endpoint does not return a total.
      params.set("limit", String(PAGE_SIZE + 1));
      params.set("offset", String(key.page * PAGE_SIZE));
      const expand = expandParam(resource);
      if (expand) params.set("expand", expand);
      if (key.search && resource.search_field) params.set(resource.search_field, key.search);

      return asRecords(
        await api(`/${resource.name}?${params.toString()}`, {
          org: resource.scope === "organization",
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

  const runSearch = () => {
    setPage(0);
    setApplied(search().trim());
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
          <Show when={props.resource.search_field}>
            <div class="w-56">
              <SearchInput
                value={search()}
                placeholder={`Search by ${searchLabel(props.resource)}…`}
                onInput={setSearch}
                onSubmit={runSearch}
              />
            </div>
          </Show>
          <Button size="sm" variant="ghost" onClick={() => void refetch()} title="Reload">
            Refresh
          </Button>
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
                  title={applied() ? "Nothing matched that search" : `No ${props.resource.plural.toLowerCase()} yet`}
                  description={
                    applied()
                      ? "Try a different search, or clear it to see everything."
                      : can(props.resource, "create")
                        ? `Create the first one to get started.`
                        : "There is nothing here for you to see yet."
                  }
                >
                  <Show when={applied()}>
                    <Button
                      onClick={() => {
                        setSearch("");
                        setApplied("");
                      }}
                    >
                      Clear search
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
                    <For each={columns()}>{(field) => <th class="px-4 py-2.5 font-semibold">{field.label}</th>}</For>
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

function searchLabel(resource: ResourceManifest): string {
  const field = resource.fields.find((entry) => entry.name === resource.search_field);
  return (field?.label ?? resource.search_field ?? "name").toLowerCase();
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
          org: resource.scope === "organization",
        }),
      );
    },
  );

  // Reset the form whenever the record (or the resource) changes underneath it.
  //
  // The rewrite must be untracked: reading the draft's own keys in order to
  // clear them would make this effect depend on the store it writes, and it
  // would re-run itself forever.
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
    // An `owner` policy is the one case the manifest alone cannot settle, so
    // check the row we actually loaded.
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
            org: props.resource.scope === "organization",
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
        org: props.resource.scope === "organization",
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
                <div class="grid gap-4 px-5 py-5 sm:grid-cols-2">
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
              <dd class="mt-0.5 text-muted">{formatValue(field, props.record)}</dd>
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
 * Records that point at this one — an order's lines, a customer's orders.
 * Shown inline because "what is attached to this?" is the question a record
 * screen exists to answer.
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
      // `via` names which reference to follow, which matters when the child
      // points at the same parent more than once (billing vs shipping address).
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
