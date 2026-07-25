export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

export interface AuthManifest {
  identity_field: string;
  allow_registration: boolean;
}

export interface ActionPermissionManifest {
  value: string;
  note: string;
  requires_org: boolean;
}

export interface ActionPermissionsManifest {
  list: ActionPermissionManifest;
  read: ActionPermissionManifest;
  create: ActionPermissionManifest;
  update: ActionPermissionManifest;
  delete: ActionPermissionManifest;
}

export interface FieldManifest {
  name: string;
  type: "string" | "text" | "integer" | "big_int" | "float" | "boolean" | "uuid" | "timestamp" | "json" | "reference";
  required: boolean;
  unique: boolean;
  hidden: boolean;
  references: string | null;
  relation: string | null;
  on_delete: "restrict" | "set_null" | "cascade" | "no_action" | null;
  default_value: JsonValue;
  writable: boolean;
}

export interface RelationManifest {
  field: string;
  relation: string;
  target: string;
}

export interface ResourceManifest {
  name: string;
  builtin: boolean;
  scope: "organization" | "global";
  owner_field: string;
  fields: FieldManifest[];
  relations: RelationManifest[];
  permissions: ActionPermissionsManifest;
  permission_summary: string;
  endpoint_summary: string;
}

export interface FunctionManifest {
  name: string;
  description: string;
  method: "GET" | "POST" | "PUT" | "DELETE";
  visibility: "public" | "authenticated" | "role" | "private";
  visibility_label: string;
  role: string | null;
  note: string;
}

export interface AdminManifest {
  title: string;
  app_name: string;
  api_base_url: string;
  docs_url: string | null;
  auth: AuthManifest;
  resources: ResourceManifest[];
  functions: FunctionManifest[];
}

export type AuthMode = "bearer" | "apiKey";
export type NoticeKind = "success" | "error" | "warn" | "info";
export type Page = { kind: "auth" | "dashboard" | "organization" | "resource" | "function"; name: string | null };

export interface Notice {
  kind: NoticeKind;
  message: string;
}

export type ApiRecord = Record<string, unknown>;

export interface ResourceState {
  loading: boolean;
  saving: boolean;
  error: string | null;
  rows: ApiRecord[];
  selectedId: string;
  selectedRecord: ApiRecord | null;
  formDraft: Record<string, string | boolean>;
  filterField: string;
  filterValue: string;
  limit: string;
  offset: string;
}

export interface FunctionState {
  input: string;
  loading: boolean;
  error: string | null;
  output: unknown;
}

export interface InviteLookup {
  id: string;
  label: string;
}

export interface AdminState {
  manifest: AdminManifest | null;
  page: Page;
  notice: Notice | null;
  auth: {
    mode: AuthMode;
    bearerToken: string;
    apiKey: string;
    userId: string | null;
    profile: ApiRecord | null;
    organizations: ApiRecord[];
    selectedOrgId: string;
    role: string | null;
    refreshing: boolean;
  };
  forms: {
    loginIdentity: string;
    loginPassword: string;
    registerIdentity: string;
    registerPassword: string;
    registerExtra: string;
    manualBearerToken: string;
    manualApiKey: string;
    createOrgName: string;
    createOrgSlug: string;
    editOrgName: string;
    editOrgSlug: string;
    inviteIdentity: string;
    inviteUserId: string;
    inviteRole: string;
  };
  organizations: {
    loadingMembers: boolean;
    membersError: string | null;
    members: ApiRecord[];
    memberRoleDrafts: Record<string, string>;
    inviteLookup: InviteLookup | null;
    inviteLookupError: string | null;
    inviteLookupLoading: boolean;
  };
  resources: Record<string, ResourceState>;
  functions: Record<string, FunctionState>;
}
