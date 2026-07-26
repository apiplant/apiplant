/**
 * The auth resources, managed as the things they are.
 *
 * `user`, `organization`, `membership` and `api_key` are ordinary resources to
 * the API, but a table of `membership` rows with a free-text `role` column and
 * a `user_id` foreign key is a developer's view of a team. So they get screens
 * built around the job instead: your account, your team, your organization,
 * your API keys. The generic table is still available to an app that
 * deliberately turns it back on with `[admin] visible = true`.
 */

import { For, Show, createEffect, createMemo, createResource, createSignal, untrack } from "solid-js";
import { createMutable } from "solid-js/store";
import {
  Avatar,
  Badge,
  Button,
  Card,
  CardHeader,
  ConfirmDialog,
  Dialog,
  EmptyState,
  Field,
  PageTitle,
} from "../ui";
import { FieldEditor, buildPayload, createDraft, recordLabel } from "../fields";
import type { Draft } from "../fields";
import {
  api,
  asRecord,
  asRecords,
  currentOrganization,
  currentUserLabel,
  hasRole,
  manifest,
  notify,
  organizationLabel,
  refreshSession,
  reportError,
  refreshRole,
  resourceByName,
  session,
  setActiveOrganization,
} from "../store";
import type { ApiRecord, ResourceManifest } from "../types";

/** A stand-in resource so the shared form machinery can edit a profile. */
function profileResource(): ResourceManifest {
  const real = resourceByName("user");
  const fields = manifest()?.auth.profile_fields ?? [];
  return {
    ...(real ?? ({} as ResourceManifest)),
    name: "user",
    label: "Profile",
    plural: "Profiles",
    scope: "global",
    fields,
    relations: [],
    children: [],
    columns: [],
  };
}

// --- account ---------------------------------------------------------------

export function AccountPage() {
  const resource = createMemo(profileResource);
  const draft = createMutable<Draft>({});
  const [saving, setSaving] = createSignal(false);

  // Untracked for the same reason as the record form: clearing the draft reads
  // its keys, and a tracked read of what this effect writes never settles.
  createEffect(() => {
    const fresh = createDraft(resource(), session.profile);
    untrack(() => {
      for (const key of Object.keys(draft)) delete draft[key];
      Object.assign(draft, fresh);
    });
  });

  const save = async () => {
    if (!session.userId) return;
    const { payload, errors } = buildPayload(resource(), draft);
    if (errors.length) {
      notify("error", errors[0].message);
      return;
    }
    setSaving(true);
    try {
      await api(`/user/${encodeURIComponent(session.userId)}`, { method: "PATCH", body: payload });
      await refreshSession();
      notify("success", "Your details are saved.");
    } catch (error) {
      reportError(error);
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <PageTitle title="Your account" subtitle="The details other people see, and how you sign in.">
        <Button variant="primary" loading={saving()} onClick={() => void save()}>
          Save changes
        </Button>
      </PageTitle>

      <div class="grid gap-4 xl:grid-cols-[minmax(0,1fr)_20rem]">
        <Card>
          <CardHeader title="Profile" />
          <div class="grid gap-4 px-5 py-5 sm:grid-cols-2">
            <For each={resource().fields.filter((field) => field.writable)}>
              {(field) => (
                <div class={field.widget === "textarea" || field.widget === "json" ? "sm:col-span-2" : ""}>
                  <FieldEditor field={field} draft={draft} />
                </div>
              )}
            </For>
          </div>
        </Card>

        <Card>
          <CardHeader title="Signed in as" />
          <div class="flex items-center gap-3 px-5 py-5">
            <Avatar name={currentUserLabel()} />
            <div class="min-w-0">
              <p class="truncate text-sm font-medium text-ink">{currentUserLabel()}</p>
              <p class="mt-0.5 text-xs text-muted">
                {session.roles.length
                  ? `${session.roles.join(", ")} in ${organizationLabel(currentOrganization())}`
                  : "No role assigned"}
              </p>
            </div>
          </div>
        </Card>
      </div>
    </>
  );
}

// --- team ------------------------------------------------------------------

export function TeamPage() {
  const [inviting, setInviting] = createSignal(false);
  const [removing, setRemoving] = createSignal<ApiRecord | null>(null);
  const [busy, setBusy] = createSignal(false);

  const [members, { refetch }] = createResource(
    () => session.organizationId,
    async (organizationId) => {
      if (!organizationId) return [];
      const rows = asRecords(await api("/membership?limit=200&expand=user", { org: true }));
      // Roles live in two places — the membership's primary one and its
      // `membership_role` rows — so the screen stitches them back together the
      // same way the server does when it checks a permission.
      const grants = asRecords(
        await api("/membership_role?limit=500", { org: true }).catch(() => []),
      );
      return rows.map(
        (member): ApiRecord => ({
          ...member,
          grants: grants.filter(
            (grant) => String(grant.membership_id ?? "") === String(member.id ?? ""),
          ),
        }),
      );
    },
  );

  const roles = () => manifest()?.auth.known_roles ?? ["member", "admin"];
  const membership = createMemo(() => resourceByName("membership"));
  const mayManage = createMemo(() => {
    const policy = membership()?.permissions.create;
    if (!policy) return false;
    return policy.role ? hasRole(policy.role) : policy.value !== "private";
  });

  const memberName = (member: ApiRecord) => {
    const user = asRecord(member.user);
    const identity = manifest()?.auth.identity_field;
    if (user && identity && typeof user[identity] === "string") return String(user[identity]);
    if (user) return recordLabel(resourceByName("user"), user);
    return String(member.user_id ?? "Member");
  };

  const isMe = (member: ApiRecord) => String(member.user_id ?? "") === session.userId;

  /** Every role a member holds: the primary one first, then their grants. */
  const rolesOf = (member: ApiRecord): string[] => {
    const grants = Array.isArray(member.grants) ? (member.grants as ApiRecord[]) : [];
    return [...new Set([String(member.role ?? ""), ...grants.map((g) => String(g.role ?? ""))])].filter(
      Boolean,
    );
  };

  /** Roles left to give someone, so the picker never offers a duplicate. */
  const grantable = (member: ApiRecord) => roles().filter((role) => !rolesOf(member).includes(role));

  /**
   * Whether this role may be taken away here.
   *
   * Nobody may remove their own `admin`: an organisation can only lose its last
   * administrator if that administrator removes themselves, so refusing it is
   * what keeps every organisation administrable. Another admin still can.
   */
  const mayRevoke = (member: ApiRecord, role: string) => !(isMe(member) && role === "admin");

  /** Re-read our own permissions when we changed our own roles. */
  const afterChange = async (member: ApiRecord) => {
    void refetch();
    if (isMe(member)) await refreshRole();
  };

  const changeRole = async (member: ApiRecord, role: string) => {
    try {
      await api(`/membership/${encodeURIComponent(String(member.id ?? ""))}`, {
        method: "PATCH",
        body: { role },
        org: true,
      });
      notify("success", `${memberName(member)} is now ${role}.`);
      await afterChange(member);
    } catch (error) {
      reportError(error);
    }
  };

  const grantRole = async (member: ApiRecord, role: string) => {
    if (!role) return;
    try {
      await api("/membership_role", {
        method: "POST",
        body: { membership_id: String(member.id ?? ""), role },
        org: true,
      });
      notify("success", `${memberName(member)} is also ${role}.`);
      await afterChange(member);
    } catch (error) {
      reportError(error);
    }
  };

  const revokeRole = async (member: ApiRecord, role: string) => {
    const grant = (Array.isArray(member.grants) ? (member.grants as ApiRecord[]) : []).find(
      (candidate) => String(candidate.role ?? "") === role,
    );
    try {
      if (grant) {
        await api(`/membership_role/${encodeURIComponent(String(grant.id ?? ""))}`, {
          method: "DELETE",
          org: true,
        });
      } else {
        // The primary role has no grant row to delete; clearing the column is
        // how it goes away.
        await api(`/membership/${encodeURIComponent(String(member.id ?? ""))}`, {
          method: "PATCH",
          body: { role: null },
          org: true,
        });
      }
      notify("success", `${memberName(member)} is no longer ${role}.`);
      await afterChange(member);
    } catch (error) {
      reportError(error);
    }
  };

  const remove = async () => {
    const member = removing();
    if (!member) return;
    setBusy(true);
    try {
      await api(`/membership/${encodeURIComponent(String(member.id ?? ""))}`, {
        method: "DELETE",
        org: true,
      });
      notify("success", `${memberName(member)} no longer has access.`);
      void refetch();
    } catch (error) {
      reportError(error);
    } finally {
      setBusy(false);
      setRemoving(null);
    }
  };

  return (
    <>
      <PageTitle
        title="Team"
        subtitle={`Who can work in ${organizationLabel(currentOrganization())}, and what they may do.`}
      >
        <Show when={mayManage() && session.organizationId}>
          <Button variant="primary" onClick={() => setInviting(true)}>
            Add someone
          </Button>
        </Show>
      </PageTitle>

      <Show
        when={session.organizationId}
        fallback={
          <EmptyState
            title="No organization selected"
            description="Choose or create an organization first — a team belongs to one."
          />
        }
      >
        <Card class="overflow-hidden">
          <CardHeader title="People" hint={`${(members() ?? []).length} with access`} />
          <Show
            when={!members.loading}
            fallback={<p class="px-5 py-6 text-xs text-faint">Loading…</p>}
          >
            <Show
              when={(members() ?? []).length}
              fallback={
                <div class="px-5 py-4">
                  <EmptyState
                    title="Nobody here yet"
                    description="Add a teammate by the address they signed up with."
                  />
                </div>
              }
            >
              <ul class="divide-y divide-line">
                <For each={members()}>
                  {(member) => (
                    <li class="flex flex-wrap items-center gap-3 px-5 py-3">
                      <Avatar name={memberName(member)} />
                      <div class="min-w-0 flex-1">
                        <p class="truncate text-sm font-medium text-ink">{memberName(member)}</p>
                        <Show when={String(member.user_id ?? "") === session.userId}>
                          <p class="text-[0.6875rem] text-faint">This is you</p>
                        </Show>
                      </div>
                      <Show
                        when={mayManage()}
                        fallback={
                          <div class="flex flex-wrap gap-1">
                            <Show
                              when={rolesOf(member).length}
                              fallback={<Badge>no role</Badge>}
                            >
                              <For each={rolesOf(member)}>
                                {(role) => <Badge tone={role === "admin" ? "accent" : undefined}>{role}</Badge>}
                              </For>
                            </Show>
                          </div>
                        }
                      >
                        <div class="flex flex-wrap items-center gap-1.5">
                          <For each={rolesOf(member)}>
                            {(role) => (
                              <span class="inline-flex items-center gap-1 rounded-full border border-line bg-surface-2/60 py-0.5 pl-2.5 pr-1 text-[0.6875rem] text-ink">
                                {role}
                                <Show
                                  when={mayRevoke(member, role)}
                                  fallback={
                                    <span
                                      class="px-1 text-faint"
                                      title="You cannot remove your own admin role. Another admin can."
                                      aria-label="Your own admin role cannot be removed"
                                    >
                                      ·
                                    </span>
                                  }
                                >
                                  <button
                                    type="button"
                                    class="rounded-full px-1 text-faint transition-colors hover:text-danger"
                                    title={`Remove ${role}`}
                                    aria-label={`Remove ${role} from ${memberName(member)}`}
                                    onClick={() => void revokeRole(member, role)}
                                  >
                                    ×
                                  </button>
                                </Show>
                              </span>
                            )}
                          </For>
                          <Show when={grantable(member).length}>
                            <select
                              class="input w-28 py-1 text-[0.6875rem]"
                              value=""
                              onChange={(event) => {
                                const role = event.currentTarget.value;
                                event.currentTarget.value = "";
                                void (rolesOf(member).length
                                  ? grantRole(member, role)
                                  : changeRole(member, role));
                              }}
                            >
                              <option value="">Add role…</option>
                              <For each={grantable(member)}>
                                {(role) => <option value={role}>{role}</option>}
                              </For>
                            </select>
                          </Show>
                          <Show
                            when={!isMe(member) || !rolesOf(member).includes("admin")}
                            fallback={<span class="w-[4.5rem]" />}
                          >
                            <Button variant="ghost" size="sm" onClick={() => setRemoving(member)}>
                              Remove
                            </Button>
                          </Show>
                        </div>
                      </Show>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </Show>
        </Card>
      </Show>

      <InviteDialog
        open={inviting()}
        roles={roles()}
        onClose={() => setInviting(false)}
        onAdded={() => {
          setInviting(false);
          void refetch();
        }}
      />

      <ConfirmDialog
        open={Boolean(removing())}
        title="Remove from this organization?"
        description={`${removing() ? memberName(removing()!) : "This person"} will lose access to everything in ${organizationLabel(
          currentOrganization(),
        )}. Their account itself is not deleted.`}
        confirmLabel="Remove"
        danger
        busy={busy()}
        onConfirm={() => void remove()}
        onCancel={() => setRemoving(null)}
      />
    </>
  );
}

/**
 * Adding a teammate is one call: the membership is created from the person's
 * identity, and the server resolves it to an account.
 *
 * Looking the user up from here would not work — a member may only read users
 * they already share an organization with, which the person being added is by
 * definition not. The `organization_join` hook on `membership` does it instead,
 * and answers 404 when nobody is registered with that identity.
 */
function InviteDialog(props: { open: boolean; roles: string[]; onClose: () => void; onAdded: () => void }) {
  const [identity, setIdentity] = createSignal("");
  const [role, setRole] = createSignal("member");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const label = () => manifest()?.auth.identity_label ?? "Email";

  const add = async () => {
    const value = identity().trim();
    if (!value) {
      setError(`Enter their ${label().toLowerCase()}.`);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const field = manifest()!.auth.identity_field;
      await api("/membership", {
        method: "POST",
        body: { [field]: value, role: role() },
        org: true,
      });
      notify("success", `${value} can now work in ${organizationLabel(currentOrganization())}.`);
      setIdentity("");
      setRole("member");
      props.onAdded();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={props.open}
      title="Add someone to this organization"
      description="They need an account already. Adding them here is what gives it access."
      onClose={props.onClose}
      footer={
        <>
          <Button variant="ghost" onClick={props.onClose}>
            Cancel
          </Button>
          <Button variant="primary" loading={busy()} onClick={() => void add()}>
            Add to organization
          </Button>
        </>
      }
    >
      <div class="space-y-4">
        <Field label={label()} required>
          <input
            class="input"
            value={identity()}
            onInput={(event) => setIdentity(event.currentTarget.value)}
          />
        </Field>
        <Field label="Role" help="Roles decide what someone may do here.">
          <select class="input" value={role()} onChange={(event) => setRole(event.currentTarget.value)}>
            <For each={props.roles}>{(entry) => <option value={entry}>{entry}</option>}</For>
          </select>
        </Field>
        <Show when={error()}>
          <p class="rounded-lg border border-danger-line bg-danger-soft px-3 py-2 text-[0.8125rem] text-ink">
            {error()}
          </p>
        </Show>
      </div>
    </Dialog>
  );
}

// --- organization ----------------------------------------------------------

export function OrganizationPage() {
  const [name, setName] = createSignal("");
  const [slug, setSlug] = createSignal("");
  const [saving, setSaving] = createSignal(false);
  const [creating, setCreating] = createSignal(false);

  createEffect(() => {
    const current = currentOrganization();
    setName(typeof current?.name === "string" ? current.name : "");
    setSlug(typeof current?.slug === "string" ? current.slug : "");
  });

  const mayEdit = createMemo(() => {
    const policy = resourceByName("organization")?.permissions.update;
    if (!policy) return false;
    return policy.role ? hasRole(policy.role) : policy.value !== "private";
  });

  const save = async () => {
    const current = currentOrganization();
    if (!current) return;
    setSaving(true);
    try {
      await api(`/organization/${encodeURIComponent(String(current.id ?? ""))}`, {
        method: "PATCH",
        body: { name: name().trim(), slug: slug().trim() || null },
      });
      await refreshSession();
      notify("success", "Organization updated.");
    } catch (error) {
      reportError(error);
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <PageTitle title="Organization" subtitle="The workspace everything you create belongs to.">
        <Button variant="primary" onClick={() => setCreating(true)}>
          New organization
        </Button>
      </PageTitle>

      <div class="grid gap-4 xl:grid-cols-2">
        <Card>
          <CardHeader title="Details" />
          <Show
            when={currentOrganization()}
            fallback={
              <div class="px-5 py-4">
                <EmptyState
                  title="No organization yet"
                  description="Create one to start working. Whoever creates it becomes its admin."
                >
                  <Button variant="primary" onClick={() => setCreating(true)}>
                    Create organization
                  </Button>
                </EmptyState>
              </div>
            }
          >
            <div class="space-y-4 px-5 py-5">
              <Field label="Name" required>
                <input
                  class="input"
                  disabled={!mayEdit()}
                  value={name()}
                  onInput={(event) => setName(event.currentTarget.value)}
                />
              </Field>
              <Field label="Short name" help="A short, unique handle used in links.">
                <input
                  class="input"
                  disabled={!mayEdit()}
                  value={slug()}
                  onInput={(event) => setSlug(event.currentTarget.value)}
                />
              </Field>
              <Show
                when={mayEdit()}
                fallback={
                  <p class="text-xs text-faint">Only an admin of this organization can change these.</p>
                }
              >
                <Button variant="primary" loading={saving()} onClick={() => void save()}>
                  Save changes
                </Button>
              </Show>
            </div>
          </Show>
        </Card>

        <Card>
          <CardHeader title="Your organizations" hint="Switch at any time from the top bar." />
          <Show
            when={session.organizations.length}
            fallback={<p class="px-5 py-4 text-xs text-faint">You do not belong to any yet.</p>}
          >
            <ul class="divide-y divide-line">
              <For each={session.organizations}>
                {(organization) => {
                  const id = String(organization.id ?? "");
                  return (
                    <li>
                      <button
                        type="button"
                        class="flex w-full items-center gap-3 px-5 py-3 text-left transition-colors hover:bg-surface-2/60"
                        onClick={() => void setActiveOrganization(id)}
                      >
                        <Avatar name={organizationLabel(organization)} size="sm" />
                        <span class="min-w-0 flex-1 truncate text-sm text-ink">
                          {organizationLabel(organization)}
                        </span>
                        <Show when={session.organizationId === id}>
                          <Badge tone="accent">Active</Badge>
                        </Show>
                      </button>
                    </li>
                  );
                }}
              </For>
            </ul>
          </Show>
        </Card>
      </div>

      <CreateOrganizationDialog open={creating()} onClose={() => setCreating(false)} />
    </>
  );
}

function CreateOrganizationDialog(props: { open: boolean; onClose: () => void }) {
  const [name, setName] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const create = async () => {
    if (!name().trim()) {
      setError("Give it a name.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const created = asRecord(
        await api("/organization", { method: "POST", body: { name: name().trim() } }),
      );
      await refreshSession();
      if (created?.id) await setActiveOrganization(String(created.id));
      notify("success", `${name().trim()} is ready.`);
      setName("");
      props.onClose();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={props.open}
      title="New organization"
      description="You will be its admin, and it becomes your active workspace."
      onClose={props.onClose}
      footer={
        <>
          <Button variant="ghost" onClick={props.onClose}>
            Cancel
          </Button>
          <Button variant="primary" loading={busy()} onClick={() => void create()}>
            Create
          </Button>
        </>
      }
    >
      <div class="space-y-4">
        <Field label="Name" required>
          <input class="input" value={name()} onInput={(event) => setName(event.currentTarget.value)} />
        </Field>
        <Show when={error()}>
          <p class="rounded-lg border border-danger-line bg-danger-soft px-3 py-2 text-[0.8125rem] text-ink">
            {error()}
          </p>
        </Show>
      </div>
    </Dialog>
  );
}

// --- API keys --------------------------------------------------------------

export function ApiKeysPage() {
  const [creating, setCreating] = createSignal(false);
  const [keyName, setKeyName] = createSignal("");
  const [issued, setIssued] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [revoking, setRevoking] = createSignal<ApiRecord | null>(null);

  const [keys, { refetch }] = createResource(
    () => session.userId,
    async () => asRecords(await api("/api_key?limit=100")),
  );

  const create = async () => {
    setBusy(true);
    try {
      const response = asRecord(
        await api("/auth/apikeys", { method: "POST", body: { name: keyName().trim() || "Untitled key" } }),
      );
      const secret = typeof response?.api_key === "string" ? response.api_key : null;
      setIssued(secret);
      setCreating(false);
      setKeyName("");
      void refetch();
    } catch (error) {
      reportError(error);
    } finally {
      setBusy(false);
    }
  };

  const revoke = async () => {
    const key = revoking();
    if (!key) return;
    setBusy(true);
    try {
      await api(`/api_key/${encodeURIComponent(String(key.id ?? ""))}`, { method: "DELETE" });
      notify("success", "Key revoked.");
      void refetch();
    } catch (error) {
      reportError(error);
    } finally {
      setBusy(false);
      setRevoking(null);
    }
  };

  return (
    <>
      <PageTitle
        title="API keys"
        subtitle="Let a script or another system act on your behalf."
      >
        <Button variant="primary" onClick={() => setCreating(true)}>
          New key
        </Button>
      </PageTitle>

      <Card class="overflow-hidden">
        <CardHeader title="Your keys" />
        <Show when={!keys.loading} fallback={<p class="px-5 py-6 text-xs text-faint">Loading…</p>}>
          <Show
            when={(keys() ?? []).length}
            fallback={
              <div class="px-5 py-4">
                <EmptyState
                  title="No keys yet"
                  description="A key acts as you, with everything you can do. Create one only when something needs it."
                />
              </div>
            }
          >
            <ul class="divide-y divide-line">
              <For each={keys()}>
                {(key) => (
                  <li class="flex items-center gap-3 px-5 py-3">
                    <div class="min-w-0 flex-1">
                      <p class="truncate text-sm font-medium text-ink">{String(key.name ?? "Untitled key")}</p>
                      <Show when={typeof key.created_at === "string"}>
                        <p class="mt-0.5 text-[0.6875rem] text-faint">
                          Created {new Date(String(key.created_at)).toLocaleDateString(undefined, { dateStyle: "medium" })}
                        </p>
                      </Show>
                    </div>
                    <Button variant="ghost" size="sm" onClick={() => setRevoking(key)}>
                      Revoke
                    </Button>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </Show>
      </Card>

      <Dialog
        open={creating()}
        title="New API key"
        description="Name it after whatever will use it, so you know what you are revoking later."
        onClose={() => setCreating(false)}
        footer={
          <>
            <Button variant="ghost" onClick={() => setCreating(false)}>
              Cancel
            </Button>
            <Button variant="primary" loading={busy()} onClick={() => void create()}>
              Create key
            </Button>
          </>
        }
      >
        <Field label="Name">
          <input
            class="input"
            placeholder="Nightly import"
            value={keyName()}
            onInput={(event) => setKeyName(event.currentTarget.value)}
          />
        </Field>
      </Dialog>

      {/* The plaintext key exists exactly once, in this response. */}
      <Dialog
        open={Boolean(issued())}
        title="Copy your key now"
        description="This is the only time it is shown. If you lose it, revoke the key and make another."
        onClose={() => setIssued(null)}
        footer={
          <>
            <Button
              onClick={() => {
                void navigator.clipboard?.writeText(issued() ?? "");
                notify("success", "Copied to your clipboard.");
              }}
            >
              Copy
            </Button>
            <Button variant="primary" onClick={() => setIssued(null)}>
              Done
            </Button>
          </>
        }
      >
        <code class="block break-all rounded-xl border border-line bg-surface-2 px-3.5 py-3 font-mono text-[0.78125rem] text-ink">
          {issued()}
        </code>
      </Dialog>

      <ConfirmDialog
        open={Boolean(revoking())}
        title="Revoke this key?"
        description={`Anything still using “${String(revoking()?.name ?? "this key")}” will stop working immediately.`}
        confirmLabel="Revoke"
        danger
        busy={busy()}
        onConfirm={() => void revoke()}
        onCancel={() => setRevoking(null)}
      />
    </>
  );
}
