/**
 * The auth resources, managed as the things they are.
 *
 * `user`, `organization`, `membership` and `api_key` are ordinary resources to
 * the API, but a table of `membership` rows with a free-text `role` column and
 * a `user_id` foreign key is a developer's view of a team. They therefore get
 * task-oriented screens instead: your account, your team, your organization and
 * your API keys. The generic table remains available to an app that enables it
 * with `[admin] visible = true`.
 */

import { For, Show, createEffect, createMemo, createSignal, isPending, latest, refresh } from "solid-js";
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
import { FieldEditor, FilePicker, buildPayload, createDraft, createDraftStore, recordLabel } from "../fields";
import {
  api,
  asRecord,
  asRecords,
  avatarOf,
  connectOAuth,
  currentOrganization,
  currentUserAvatar,
  currentUserEmail,
  currentUserLabel,
  emailOf,
  hasRole,
  canPermission,
  manifest,
  notify,
  oauthAvailable,
  organizationLabel,
  refreshSession,
  reportError,
  refreshRole,
  resourceByName,
  session,
  setActiveOrganization,
} from "../store";
import { ProviderMark } from "../brand-icons";
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
  const draft = createDraftStore();
  const [saving, setSaving] = createSignal(false);

  // The apply phase is untracked, so writing the draft cannot feed back into
  // the compute that decided to write it.
  createEffect(
    () => [resource(), session.profile] as const,
    ([manifest, profile]) => {
      draft.reset(createDraft(manifest, profile));
    },
  );

  const save = async () => {
    if (!session.userId) return;
    const { payload, errors } = buildPayload(resource(), draft.values);
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

        <div class="space-y-4">
          <Show when={oauthAvailable()}>
            <LinkedAccountsCard />
          </Show>

          <Card>
          <CardHeader title="Signed in as" />
          <div class="flex items-center gap-3 px-5 py-5">
            <Avatar name={currentUserLabel()} src={currentUserAvatar()} email={currentUserEmail()} />
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
      </div>
    </>
  );
}

/**
 * The providers this account can sign in through.
 *
 * Both halves of the same list: what is connected, and what could be. The
 * server refuses to remove the last way into an account — no password, no other
 * provider — and says so, which is why this screen does not try to work that
 * out for itself: it asks, and shows the answer.
 */
function LinkedAccountsCard() {
  const providers = () => manifest()?.auth.oauth_providers ?? [];
  const connectionsResource = createMemo(async () =>
    asRecords(await api("/oauth_connection?limit=50")),
  );
  const connections = () => latest(connectionsResource) ?? [];
  const refetch = () => refresh(connectionsResource);
  const [busy, setBusy] = createSignal("");

  const linked = (provider: string) =>
    connections().find((row) => String(row.provider ?? "") === provider) ?? null;

  const connect = async (provider: { provider: string; start_url: string }) => {
    setBusy(provider.provider);
    try {
      // Leaves the page: the browser goes to the provider and comes back to
      // `#/account`, so there is nothing after this to await.
      await connectOAuth(provider);
    } catch (error) {
      reportError(error);
      setBusy("");
    }
  };

  const disconnect = async (provider: string) => {
    setBusy(provider);
    try {
      await api(`/auth/oauth/${encodeURIComponent(provider)}`, { method: "DELETE" });
      notify("success", `${provider} is no longer linked to your account.`);
      refetch();
    } catch (error) {
      reportError(error);
    } finally {
      setBusy("");
    }
  };

  return (
    <Card>
      <CardHeader title="Linked accounts" hint="Sign in with a provider instead of a password." />
      <div class="divide-y divide-line">
        <For each={providers()}>
          {(provider) => {
            const connection = () => linked(provider.provider);
            return (
              <div class="flex items-center gap-3 px-5 py-3.5">
                <ProviderMark provider={provider.provider} label={provider.label} icon={provider.icon} />
                <div class="min-w-0 flex-1">
                  <p class="truncate text-[0.8125rem] font-medium text-ink">{provider.label}</p>
                  <Show
                    when={connection()}
                    fallback={<p class="mt-0.5 text-xs text-faint">Not connected</p>}
                  >
                    {/* Whatever the provider last called them, which is the
                        useful answer to "which account is this?" */}
                    <p class="mt-0.5 truncate text-xs text-muted">
                      {String(
                        connection()!.email ??
                          connection()!.display_name ??
                          connection()!.provider_user_id ??
                          "Connected",
                      )}
                    </p>
                  </Show>
                </div>
                <Show
                  when={connection()}
                  fallback={
                    <Button
                      size="sm"
                      variant="ghost"
                      loading={busy() === provider.provider}
                      onClick={() => void connect(provider)}
                    >
                      Connect
                    </Button>
                  }
                >
                  <Button
                    size="sm"
                    variant="ghost"
                    loading={busy() === provider.provider}
                    onClick={() => void disconnect(provider.provider)}
                  >
                    Disconnect
                  </Button>
                </Show>
              </div>
            );
          }}
        </For>
      </div>
    </Card>
  );
}

// --- team ------------------------------------------------------------------

export function TeamPage() {
  const [inviting, setInviting] = createSignal(false);
  const [removing, setRemoving] = createSignal<ApiRecord | null>(null);
  const [busy, setBusy] = createSignal(false);

  const membersResource = createMemo(async () => {
      const organizationId = session.organizationId;
      if (!organizationId) return [];
      const rows = asRecords(await api("/membership?limit=200&expand=user", { org: true }));
      // Roles live in two places, the membership's primary role and its
      // `membership_role` rows, so the screen combines them the same way the
      // server does when checking a permission.
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
  });
  const members = () => latest(membersResource) ?? [];
  const membersLoading = () => isPending(membersResource);
  const refetch = () => refresh(membersResource);

  // Invitations sent and not yet accepted. Only fetched where the feature
  // exists, since without a mail provider there are none.
  const invitationsResource = createMemo(async () => {
    const organizationId = manifest()?.auth.invitations_enabled ? session.organizationId : "";
    if (!organizationId) return [];
    return asRecords(await api("/invitation?limit=200", { org: true }).catch(() => [])).filter(
      (invitation) => !invitation.accepted_at,
    );
  });
  const invitations = () => latest(invitationsResource) ?? [];
  const refetchInvitations = () => refresh(invitationsResource);

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
   * administrator if that administrator removes themselves, so refusing this
   * guarantees every organisation retains an administrator. Another admin can
   * still do it.
   */
  const mayRevoke = (member: ApiRecord, role: string) => !(isMe(member) && role === "admin");

  /** Re-read our own permissions when we changed our own roles. */
  const afterChange = async (member: ApiRecord) => {
    refetch();
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
        // The primary role has no grant row to delete, so it is removed by
        // clearing the column.
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

  /** Revoke a pending invitation — the emailed link stops working at once. */
  const revokeInvitation = async (invitation: ApiRecord) => {
    try {
      await api(`/invitation/${encodeURIComponent(String(invitation.id ?? ""))}`, {
        method: "DELETE",
        org: true,
      });
      notify("success", `The invitation to ${String(invitation.email ?? "")} was revoked.`);
      refetchInvitations();
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
      refetch();
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
            {manifest()?.auth.invitations_enabled ? "Invite someone" : "Add someone"}
          </Button>
        </Show>
      </PageTitle>

      <Show
        when={session.organizationId}
        fallback={
          <EmptyState
            title="No organization selected"
            description="Choose or create an organization first; a team belongs to one."
          />
        }
      >
        <Card class="overflow-hidden">
          <CardHeader title="People" hint={`${members().length} with access`} />
          <Show
            when={!membersLoading()}
            fallback={<p class="px-5 py-6 text-xs text-faint">Loading…</p>}
          >
            <Show
              when={members().length}
              fallback={
                <div class="px-5 py-4">
                  <EmptyState
                    title="Nobody here yet"
                    description={
                      manifest()?.auth.invitations_enabled
                        ? "Invite a teammate by email. They do not need an account first."
                        : "Add a teammate by the address they signed up with."
                    }
                  />
                </div>
              }
            >
              <ul class="divide-y divide-line">
                <For each={members()}>
                  {(member) => (
                    <li class="flex flex-wrap items-center gap-3 px-5 py-3">
                      <Avatar
                        name={memberName(member)}
                        src={avatarOf(asRecord(member.user))}
                        email={emailOf(asRecord(member.user))}
                      />
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

      {/*
        Pending invitations are not members yet, so they are a separate list
        rather than greyed-out rows among people who actually have access.
        Revoking one is deleting it: the link stops working immediately.
      */}
      <Show when={session.organizationId && invitations().length}>
        <Card class="mt-5 overflow-hidden">
          <CardHeader
            title="Invited"
            hint={`${invitations().length} waiting to accept`}
          />
          <ul class="divide-y divide-line">
            <For each={invitations()}>
              {(invitation) => (
                <li class="flex flex-wrap items-center gap-3 px-5 py-3">
                  <Avatar
                    name={String(invitation.email ?? "?")}
                    email={emailOf(invitation)}
                  />
                  <div class="min-w-0 flex-1">
                    <p class="truncate text-sm font-medium text-ink">
                      {String(invitation.email ?? "")}
                    </p>
                    <p class="text-[0.6875rem] text-faint">
                      Invited as {String(invitation.role || "member")}
                      {invitation.expires_at
                        ? ` · expires ${new Date(String(invitation.expires_at)).toLocaleDateString()}`
                        : ""}
                    </p>
                  </div>
                  <Show when={mayManage()}>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => void revokeInvitation(invitation)}
                    >
                      Revoke
                    </Button>
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Card>
      </Show>

      <InviteDialog
        open={inviting()}
        roles={roles()}
        onClose={() => setInviting(false)}
        onAdded={() => {
          setInviting(false);
          refetch();
          refetchInvitations();
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
 * Adding a teammate, using whichever mechanism this deployment supports.
 *
 * With email configured, it sends an invitation, which is the only form that
 * works for someone without an account. Without email, an organization can only
 * admit an existing account, so it falls back to creating the membership
 * directly.
 *
 * In both cases the lookup happens on the server. A member may only read users
 * they already share an organization with, which by definition excludes the
 * person being added, so the identity is resolved by the `organization_join`
 * hook on `membership` or by the invitation endpoint, and no account details
 * are returned.
 */
function InviteDialog(props: { open: boolean; roles: string[]; onClose: () => void; onAdded: () => void }) {
  const [identity, setIdentity] = createSignal("");
  const [role, setRole] = createSignal("member");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const label = () => manifest()?.auth.identity_label ?? "Email";
  const invites = () => Boolean(manifest()?.auth.invitations_enabled);

  const add = async () => {
    const value = identity().trim();
    if (!value) {
      setError(`Enter their ${label().toLowerCase()}.`);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (invites()) {
        await api("/auth/invitations", {
          method: "POST",
          body: { email: value, role: role() },
          org: true,
        });
        notify("success", `Invitation sent to ${value}.`);
      } else {
        const field = manifest()!.auth.identity_field;
        await api("/membership", {
          method: "POST",
          body: { [field]: value, role: role() },
          org: true,
        });
        notify("success", `${value} can now work in ${organizationLabel(currentOrganization())}.`);
      }
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
      title={invites() ? "Invite someone to this organization" : "Add someone to this organization"}
      description={
        invites()
          ? "We'll email them a link. They can accept it whether or not they already have an account."
          : "They need an account already. Adding them here is what gives it access."
      }
      onClose={props.onClose}
      footer={
        <>
          <Button variant="ghost" onClick={props.onClose}>
            Cancel
          </Button>
          <Button variant="primary" loading={busy()} onClick={() => void add()}>
            {invites() ? "Send invitation" : "Add to organization"}
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
  const [logo, setLogo] = createSignal("");
  const [orgClass, setOrgClass] = createSignal("");
  const [saving, setSaving] = createSignal(false);
  const [creating, setCreating] = createSignal(false);

  createEffect(
    () => currentOrganization(),
    (current) => {
      setName(typeof current?.name === "string" ? current.name : "");
      setSlug(typeof current?.slug === "string" ? current.slug : "");
      setLogo(typeof current?.avatar_url === "string" ? current.avatar_url : "");
      setOrgClass(typeof current?.org_class === "string" ? current.org_class : "");
    },
  );

  // The class is not an ordinary column: it decides which `@org_class=`
  // permissions apply inside this organisation, so the server strips it from
  // any body but an authorised one's. `[organization] org_class_editors` says
  // who that is, and the manifest carries the same policy so the field is an
  // input for them and a plain reading for everyone else.
  const classPolicy = () => manifest()?.organization?.org_class_editors ?? null;
  const mayEditClass = createMemo(() => {
    const policy = classPolicy();
    return policy ? canPermission(policy, true) : false;
  });
  const knownClasses = () => manifest()?.organization?.known_classes ?? [];

  const [scope, setScope] = createSignal<"mine" | "all">("mine");
  const [search, setSearch] = createSignal("");
  const [classSaving, setClassSaving] = createSignal<string | null>(null);

  // The server decides what this list contains: `organization` is read at
  // `member` level, so it is the caller's own organisations — unless they may
  // edit classes, in which case it is every one, because you cannot class an
  // organisation you cannot find.
  //
  // `org: true` is what makes them one: `org_class_editors` is answered against
  // the organisation *selected*, so without the header the same account is
  // nobody in particular and the list comes back narrow.
  const allOrganizationsResource = createMemo(async () =>
    mayEditClass() ? asRecords(await api("/organization?limit=500", { org: true })) : [],
  );
  const allOrganizations = () => latest(allOrganizationsResource) ?? [];
  const refetchOrganizations = () => refresh(allOrganizationsResource);

  const visibleOrganizations = createMemo(() => {
    const rows = scope() === "all" && mayEditClass() ? allOrganizations() : session.organizations;
    const needle = search().trim().toLowerCase();
    if (!needle) return rows;
    return rows.filter((organization) =>
      organizationLabel(organization).toLowerCase().includes(needle),
    );
  });

  // Sent on its own, because `org_class` is the one column the setting
  // authorises on an organisation the operator may not otherwise touch.
  const saveClass = async (id: string, value: string) => {
    setClassSaving(id);
    try {
      await api(`/organization/${encodeURIComponent(id)}`, {
        method: "PATCH",
        body: { org_class: value || null },
        org: true,
      });
      await Promise.all([refreshSession(), refetchOrganizations()]);
      notify("success", value ? `Class set to ${value}.` : "Class cleared.");
    } catch (error) {
      reportError(error);
    } finally {
      setClassSaving(null);
    }
  };

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
        body: {
          name: name().trim(),
          slug: slug().trim() || null,
          avatar_url: logo().trim() || null,
          // Sent only when this operator may write it: the server would drop
          // it otherwise, and a field nobody may change should not be part of
          // an ordinary rename.
          ...(mayEditClass() ? { org_class: orgClass().trim() || null } : {}),
        },
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
              {/* Shown beside the picker because a URL is not a picture until
                  something loads it, and a typo here is otherwise invisible
                  until the workspace switcher draws it wrong. */}
              <Field
                label="Logo"
                help="Upload a square image, or point at one. Left empty, the initials are used."
              >
                <div class="flex items-start gap-3">
                  <Avatar name={name() || organizationLabel(currentOrganization())} src={logo()} />
                  <div class="min-w-0 flex-1">
                    <FilePicker
                      value={logo()}
                      onChange={setLogo}
                      disabled={!mayEdit()}
                      placeholder="https://example.com/logo.png"
                    />
                  </div>
                </div>
              </Field>
              <Field
                label="Class"
                help={
                  mayEditClass()
                    ? "What kind of organization this is. Permissions can be narrowed to a class, so this decides what people may do here."
                    : "What kind of organization this is. Permissions can be narrowed to a class; only the operators named by `org_class_editors` can change it."
                }
              >
                <Show
                  when={mayEditClass()}
                  fallback={
                    <p class="text-xs text-faint">
                      {orgClass() ? <code class="font-mono">{orgClass()}</code> : "No class."}
                    </p>
                  }
                >
                  <input
                    class="input"
                    list="admin-org-classes"
                    placeholder="none"
                    value={orgClass()}
                    onInput={(event) => setOrgClass(event.currentTarget.value)}
                  />
                  <datalist id="admin-org-classes">
                    <For each={knownClasses()}>{(value) => <option value={value} />}</For>
                  </datalist>
                </Show>
              </Field>
              <Show
                when={mayEdit() || mayEditClass()}
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
          <CardHeader
            title="Organizations"
            hint={
              mayEditClass()
                ? "Switch at any time from the top bar. You may class any organization here, including ones you do not belong to."
                : "Switch at any time from the top bar."
            }
          />

          {/* A class editor administers classes across the deployment, so the
              server lists every organization for them. That makes the list
              long and mostly other people's, which is why the scope toggle and
              the search exist only in that case. */}
          <Show when={mayEditClass()}>
            <div class="flex flex-wrap items-center gap-2 border-b border-line px-5 py-3">
              <div class="flex rounded-md border border-line p-0.5">
                <For each={["mine", "all"] as const}>
                  {(value) => (
                    <button
                      type="button"
                      class={[
                        "rounded px-2.5 py-1 text-xs capitalize transition-colors",
                        scope() === value ? "bg-surface-2 text-ink" : "text-faint hover:text-ink",
                      ]}
                      onClick={() => setScope(value)}
                    >
                      {value}
                    </button>
                  )}
                </For>
              </div>
              <input
                class="input h-8 min-w-0 flex-1 text-xs"
                placeholder="Filter by name"
                value={search()}
                onInput={(event) => setSearch(event.currentTarget.value)}
              />
            </div>
          </Show>

          <Show
            when={visibleOrganizations().length}
            fallback={
              <p class="px-5 py-4 text-xs text-faint">
                {session.organizations.length ? "Nothing matches that." : "You do not belong to any yet."}
              </p>
            }
          >
            <ul class="divide-y divide-line">
              <For each={visibleOrganizations()}>
                {(organization) => {
                  const id = String(organization.id ?? "");
                  const mine = () => session.organizations.some((own) => String(own.id ?? "") === id);
                  const value = typeof organization.org_class === "string" ? organization.org_class : "";
                  const [draft, setDraft] = createSignal(value);
                  return (
                    <li class="px-5 py-3">
                      <div class="flex items-center gap-3">
                        <Avatar
                          name={organizationLabel(organization)}
                          src={avatarOf(organization)}
                          size="sm"
                        />
                        {/* Only an organization you belong to can be made
                            active: `X-Organization` naming any other is
                            refused, so offering the switch would be a button
                            that only ever fails. */}
                        <Show
                          when={mine()}
                          fallback={
                            <span class="min-w-0 flex-1 truncate text-sm text-ink">
                              {organizationLabel(organization)}
                            </span>
                          }
                        >
                          <button
                            type="button"
                            class="min-w-0 flex-1 truncate text-left text-sm text-ink transition-colors hover:text-accent"
                            onClick={() => void setActiveOrganization(id)}
                          >
                            {organizationLabel(organization)}
                          </button>
                        </Show>
                        <Show when={session.organizationId === id}>
                          <Badge tone="accent">Active</Badge>
                        </Show>
                        <Show when={mayEditClass() && !mine()}>
                          <Badge>Not a member</Badge>
                        </Show>
                      </div>
                      <Show
                        when={mayEditClass()}
                        fallback={
                          <Show when={value}>
                            <p class="mt-1 pl-11 text-[0.6875rem] text-faint">
                              Class <code class="font-mono">{value}</code>
                            </p>
                          </Show>
                        }
                      >
                        <div class="mt-2 flex items-center gap-2 pl-11">
                          <input
                            class="input h-8 max-w-[12rem] text-xs"
                            list="admin-org-classes"
                            placeholder="no class"
                            value={draft()}
                            onInput={(event) => setDraft(event.currentTarget.value)}
                          />
                          <Button
                            variant="ghost"
                            disabled={draft().trim() === value}
                            loading={classSaving() === id}
                            onClick={() => void saveClass(id, draft().trim())}
                          >
                            Save class
                          </Button>
                        </div>
                      </Show>
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

  const keysResource = createMemo(async () => {
    if (!session.userId) return [];
    return asRecords(await api("/api_key?limit=100"));
  });
  const keys = () => latest(keysResource) ?? [];
  const keysLoading = () => isPending(keysResource);
  const refetch = () => refresh(keysResource);

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
      refetch();
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
      refetch();
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
        <Show when={!keysLoading()} fallback={<p class="px-5 py-6 text-xs text-faint">Loading…</p>}>
          <Show
            when={keys().length}
            fallback={
              <div class="px-5 py-4">
                <EmptyState
                  title="No keys yet"
                  description="A key acts as you, with all of your permissions. Create one only when something requires it."
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
