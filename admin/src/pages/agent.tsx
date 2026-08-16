import { For, Show, createEffect, createMemo, createSignal, onSettled } from "solid-js";
import { loadConversationFilter, saveConversationFilter } from "../filters";
import { MarkupView } from "../markup";
import { Avatar, Badge, Button, ConfirmDialog, EmptyState, SearchInput, Spinner } from "../ui";
import {
  api,
  apiStream,
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
import type { AgentManifest, ApiRecord } from "../types";

interface ThreadSummary {
  id: string;
  title: string;
  updatedAt: string | null;
  createdAt: string | null;
  ownerId: string | null;
}

interface ChatEntry {
  role: "user" | "assistant" | "system" | "tool_call" | "tool_result";
  content: string;
  reasoning?: string | null;
  pending?: boolean;
  meta?: string | null;
  toolName?: string | null;
  toolCallId?: string | null;
  toolInput?: unknown;
  toolOutput?: unknown;
}

interface ToolExecution {
  key: string;
  call: ChatEntry | null;
  result: ChatEntry | null;
}

type ConversationBlock =
  | { kind: "entry"; entry: ChatEntry; index: number }
  | { kind: "tools"; entries: ChatEntry[]; key: string; executions: ToolExecution[] };

function formatWhen(value: string | null): string {
  if (!value) return "";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

function sortThreads(left: ThreadSummary, right: ThreadSummary): number {
  const leftWhen = left.updatedAt ?? left.createdAt ?? "";
  const rightWhen = right.updatedAt ?? right.createdAt ?? "";
  return rightWhen.localeCompare(leftWhen) || left.title.localeCompare(right.title) || left.id.localeCompare(right.id);
}

function threadFrom(row: ApiRecord, ownerField: string): ThreadSummary {
  const owner = ownerField ? row[ownerField] : null;
  return {
    id: String(row.id ?? ""),
    title: String(row.title ?? "Untitled conversation"),
    updatedAt: typeof row.updated_at === "string" ? row.updated_at : null,
    createdAt: typeof row.created_at === "string" ? row.created_at : null,
    ownerId: typeof owner === "string" ? owner : null,
  };
}

function includeAgentOrgContext(agent: AgentManifest): boolean {
  return agent.scope === "organization" || Boolean(session.organizationId && hasRole("admin") && agent.storage);
}

function messageFrom(row: ApiRecord): ChatEntry {
  const provider = typeof row.provider === "string" ? row.provider : "";
  const model = typeof row.model === "string" ? row.model : "";
  const finish = typeof row.finish_reason === "string" ? row.finish_reason : "";
  const toolName = typeof row.tool_name === "string" ? row.tool_name : "";
  const toolCallId = typeof row.tool_call_id === "string" ? row.tool_call_id : "";
  const meta = [toolName && `tool: ${toolName}`, toolCallId && `call: ${toolCallId}`, provider, model, finish]
    .filter(Boolean)
    .join(" · ");
  const role =
    row.role === "assistant"
      ? "assistant"
      : row.role === "system"
        ? "system"
        : row.role === "tool_call"
          ? "tool_call"
          : row.role === "tool_result" || row.role === "tool"
            ? "tool_result"
            : "user";
  return {
    role,
    content: typeof row.content === "string" ? row.content : "",
    reasoning: typeof row.reasoning === "string" ? row.reasoning : null,
    meta: meta || null,
    toolName: toolName || null,
    toolCallId: toolCallId || null,
    toolInput: row.tool_input,
    toolOutput: row.tool_output,
  };
}

function roleLabel(role: ChatEntry["role"], agentLabel: string): string {
  switch (role) {
    case "assistant":
      return agentLabel;
    case "system":
      return "System";
    case "tool_call":
      return "Tool call";
    case "tool_result":
      return "Tool result";
    default:
      return "You";
  }
}

function isToolEntry(entry: ChatEntry): boolean {
  return entry.role === "tool_call" || entry.role === "tool_result";
}

function maybeJson(text: string): unknown {
  const trimmed = text.trim();
  if (!trimmed) return null;
  try {
    return JSON.parse(trimmed);
  } catch {
    return null;
  }
}

function escapeHtml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function resolveToolPayload(value: unknown, fallback: string): { text: string; data?: unknown } {
  if (typeof value === "string") {
    const parsed = maybeJson(value);
    return parsed === null ? { text: value } : { text: JSON.stringify(parsed, null, 2), data: parsed };
  }
  if (value === null || value === undefined) {
    const parsed = maybeJson(fallback);
    return parsed === null ? { text: fallback } : { text: JSON.stringify(parsed, null, 2), data: parsed };
  }
  return { text: JSON.stringify(value, null, 2), data: value };
}

function displayToolPayload(value: unknown, fallback: string): string {
  return resolveToolPayload(value, fallback).text;
}

function jsonValueHtml(value: unknown, indent = 0): string {
  const pad = "  ".repeat(indent);
  const childPad = "  ".repeat(indent + 1);

  if (Array.isArray(value)) {
    if (!value.length) return '<span class="tok-punct">[ ]</span>';
    return [
      '<span class="tok-punct">[</span>',
      ...value.map((entry, index) => `${childPad}${jsonValueHtml(entry, indent + 1)}${index < value.length - 1 ? '<span class="tok-punct">,</span>' : ""}`),
      `${pad}<span class="tok-punct">]</span>`,
    ].join("\n");
  }

  if (value && typeof value === "object") {
    const entries = Object.entries(value);
    if (!entries.length) return '<span class="tok-punct">{ }</span>';
    return [
      '<span class="tok-punct">{</span>',
      ...entries.map(
        ([key, entry], index) =>
          `${childPad}<span class="tok-attr">"${escapeHtml(key)}"</span><span class="tok-punct">: </span>${jsonValueHtml(entry, indent + 1)}${index < entries.length - 1 ? '<span class="tok-punct">,</span>' : ""}`,
      ),
      `${pad}<span class="tok-punct">}</span>`,
    ].join("\n");
  }

  if (typeof value === "string") return `<span class="tok-str">"${escapeHtml(value)}"</span>`;
  if (typeof value === "number") return `<span class="tok-num">${String(value)}</span>`;
  if (typeof value === "boolean") return `<span class="tok-bool">${String(value)}</span>`;
  if (value === null) return '<span class="tok-null">null</span>';
  return escapeHtml(String(value));
}

function highlightToolPayload(value: unknown, fallback: string): string {
  const payload = resolveToolPayload(value, fallback);
  return payload.data === undefined ? escapeHtml(payload.text) : jsonValueHtml(payload.data);
}

function groupToolExecutions(entries: ChatEntry[]): ToolExecution[] {
  const executions: ToolExecution[] = [];
  const byCallId = new Map<string, ToolExecution>();

  entries.forEach((entry, index) => {
    if (entry.role === "tool_call") {
      const key = entry.toolCallId || `${entry.toolName || "tool"}-${index}`;
      const execution = { key, call: entry, result: null };
      executions.push(execution);
      if (entry.toolCallId) byCallId.set(entry.toolCallId, execution);
      return;
    }

    const key = entry.toolCallId || `result-${entry.toolName || "tool"}-${index}`;
    const match = entry.toolCallId ? byCallId.get(entry.toolCallId) : null;
    if (match && !match.result) {
      match.result = entry;
      byCallId.delete(entry.toolCallId!);
      return;
    }
    executions.push({ key, call: null, result: entry });
  });

  return executions;
}

function conversationFrom(entries: ChatEntry[]): ConversationBlock[] {
  const blocks: ConversationBlock[] = [];

  for (let index = 0; index < entries.length; index += 1) {
    const entry = entries[index];
    if (!isToolEntry(entry)) {
      blocks.push({ kind: "entry", entry, index });
      continue;
    }

    const toolEntries = [entry];
    let next = index + 1;
    while (next < entries.length && isToolEntry(entries[next])) {
      toolEntries.push(entries[next]);
      next += 1;
    }
    blocks.push({
      kind: "tools",
      entries: toolEntries,
      key: `tools:${index}:${next - 1}`,
      executions: groupToolExecutions(toolEntries),
    });
    index = next - 1;
  }

  return blocks;
}

function toolSummary(entries: ChatEntry[]): string {
  const calls = entries.filter((entry) => entry.role === "tool_call").length;
  const results = entries.filter((entry) => entry.role === "tool_result").length;
  const parts = [];
  if (calls) parts.push(`${calls} call${calls === 1 ? "" : "s"}`);
  if (results) parts.push(`${results} result${results === 1 ? "" : "s"}`);
  return parts.join(" · ");
}

function ToolPayload(props: { value: unknown; fallback: string; onCopy: () => void }) {
  return (
    <div class="rounded-xl border border-line bg-surface">
      <div class="flex items-center justify-end border-b border-line px-3 py-1.5">
        <button
          type="button"
          class="text-[0.72rem] text-faint transition-colors hover:text-ink hover:underline"
          onClick={props.onCopy}
        >
          Copy
        </button>
      </div>
      <pre
        class="code overflow-x-auto whitespace-pre-wrap break-words px-3 py-2 text-[0.75rem] leading-6 text-ink [overflow-wrap:anywhere] selection:bg-accent/35 selection:text-ink"
        innerHTML={highlightToolPayload(props.value, props.fallback)}
      />
    </div>
  );
}

export function AgentPage(props: { agent: AgentManifest; threadId: string | null }) {
  const [threads, setThreads] = createSignal<ThreadSummary[]>([]);
  const [messages, setMessages] = createSignal<ChatEntry[]>([]);
  const [scratchMessages, setScratchMessages] = createSignal<ChatEntry[]>([]);
  const [draft, setDraft] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [loadingThreads, setLoadingThreads] = createSignal(false);
  const [loadingMessages, setLoadingMessages] = createSignal(false);
  const [ephemeralThreadId, setEphemeralThreadId] = createSignal<string | null>(null);
  const [browseFailure, setBrowseFailure] = createSignal<string | null>(null);
  const [composerOpen, setComposerOpen] = createSignal(false);
  const [awayFromBottom, setAwayFromBottom] = createSignal(false);
  const [composerHeight, setComposerHeight] = createSignal(112);
  const [expandedToolGroups, setExpandedToolGroups] = createSignal<Record<string, boolean>>({});
  const [expandedReasoning, setExpandedReasoning] = createSignal<Record<string, boolean>>({});
  const [threadToDelete, setThreadToDelete] = createSignal<ThreadSummary | null>(null);
  const [deletingThreadId, setDeletingThreadId] = createSignal<string | null>(null);
  const [threadSearch, setThreadSearch] = createSignal("");
  const [appliedThreadSearch, setAppliedThreadSearch] = createSignal("");
  const [ownerThreadsOnly, setOwnerThreadsOnly] = createSignal(false);
  const [threadFiltersLoaded, setThreadFiltersLoaded] = createSignal(false);
  const compactQuery = window.matchMedia("(max-width: 1023px)");
  const [compactLayout, setCompactLayout] = createSignal(compactQuery.matches);

  let composerRef: HTMLTextAreaElement | undefined;
  let composerShellRef: HTMLDivElement | undefined;
  let messageListRef: HTMLDivElement | undefined;

  const onCompactLayoutChange = (event: MediaQueryListEvent) => setCompactLayout(event.matches);
  onSettled(() => {
    compactQuery.addEventListener("change", onCompactLayoutChange);
    return () => compactQuery.removeEventListener("change", onCompactLayoutChange);
  });

  const threadResource = createMemo(() =>
    props.agent.thread_resource ? resourceByName(props.agent.thread_resource) : null,
  );
  const messageResource = createMemo(() =>
    props.agent.message_resource ? resourceByName(props.agent.message_resource) : null,
  );
  const canBrowse = createMemo(
    () =>
      Boolean(
        props.agent.storage &&
          threadResource() &&
          messageResource() &&
          can(threadResource()!, "list") &&
          can(messageResource()!, "list"),
      ),
  );
  const canDeleteHistory = createMemo(() => {
    const resource = threadResource();
    return Boolean(props.agent.storage && resource && can(resource, "delete"));
  });
  const supportsThreadOwnerFilter = createMemo(
    () => Boolean(session.userId && threadResource()?.owner_field.trim()),
  );
  const canChooseThreadOwnerFilter = createMemo(() => hasRole("admin") && supportsThreadOwnerFilter());
  const activeThreadId = createMemo(() => props.threadId ?? ephemeralThreadId());
  const activeThread = createMemo(() => threads().find((thread) => thread.id === activeThreadId()) ?? null);
  const visibleMessages = createMemo(() => {
    if (!props.agent.storage) return scratchMessages();
    return canBrowse() && activeThreadId() ? messages() : scratchMessages();
  });
  const conversationBlocks = createMemo(() => conversationFrom(visibleMessages()));
  const chatOpen = createMemo(
    () => composerOpen() || Boolean(activeThreadId()) || visibleMessages().length > 0 || busy(),
  );
  const canSend = createMemo(() => Boolean(draft().trim()) && !busy());
  const compactComposer = createMemo(() => compactLayout() && chatOpen());
  const threadFiltersApplied = createMemo(() => Boolean(appliedThreadSearch() || ownerThreadsOnly()));
  const lastMessageKey = createMemo(() => {
    const entries = visibleMessages();
    const last = entries[entries.length - 1];
    return last
      ? `${entries.length}:${last.role}:${last.content.length}:${last.reasoning?.length ?? 0}:${last.pending ? "1" : "0"}`
      : "0";
  });
  const resendableIndex = createMemo(() => {
    if (busy()) return -1;
    const entries = visibleMessages();
    return entries[entries.length - 1]?.role === "user" ? entries.length - 1 : -1;
  });

  let threadsVersion = 0;
  let messagesVersion = 0;

  const resizeComposer = () => {
    if (!composerRef) return;
    composerRef.style.height = "0px";
    composerRef.style.height = `${Math.min(Math.max(composerRef.scrollHeight, 88), 240)}px`;
  };
  const measureComposer = () => {
    if (composerShellRef) setComposerHeight(composerShellRef.offsetHeight);
  };
  const syncComposerLayout = () => {
    resizeComposer();
    requestAnimationFrame(measureComposer);
  };

  const scrollToBottom = (behavior: ScrollBehavior = "auto") => {
    if (!messageListRef) return;
    messageListRef.scrollTo({
      top: Math.max(0, messageListRef.scrollHeight - messageListRef.clientHeight),
      behavior,
    });
    setAwayFromBottom(false);
  };
  const snapToBottom = () => {
    const align = () => {
      scrollToBottom();
      syncScrollAffordance();
    };
    queueMicrotask(() =>
      requestAnimationFrame(() => {
        align();
        requestAnimationFrame(() => {
          align();
          window.setTimeout(align, 0);
        });
      }),
    );
  };
  const syncScrollAffordance = () => {
    if (!messageListRef) {
      setAwayFromBottom(false);
      return;
    }
    setAwayFromBottom(messageListRef.scrollHeight - messageListRef.scrollTop - messageListRef.clientHeight > 24);
  };

  const toggleToolGroup = (key: string) =>
    setExpandedToolGroups((current) => ({ ...current, [key]: !current[key] }));
  const toggleReasoning = (key: string) =>
    setExpandedReasoning((current) => ({ ...current, [key]: !current[key] }));

  const copyWithSelectionFallback = (text: string) => {
    const input = document.createElement("textarea");
    input.value = text;
    input.setAttribute("readonly", "true");
    input.style.position = "fixed";
    input.style.opacity = "0";
    input.style.pointerEvents = "none";
    document.body.appendChild(input);
    input.focus();
    input.select();
    input.setSelectionRange(0, input.value.length);
    const copied = document.execCommand("copy");
    document.body.removeChild(input);
    if (!copied) throw new Error("Copy is not available in this browser context.");
  };

  const copyText = async (text: string) => {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(text);
      } else {
        copyWithSelectionFallback(text);
      }
      notify("success", "Copied to your clipboard.");
    } catch (error) {
      reportError(error);
    }
  };

  onSettled(() => {
    const resizeObserver = new ResizeObserver(() => {
      measureComposer();
      if (!awayFromBottom()) snapToBottom();
    });
    if (composerShellRef) resizeObserver.observe(composerShellRef);

    const syncViewport = () => {
      if (awayFromBottom()) {
        syncScrollAffordance();
      } else {
        snapToBottom();
      }
    };
    window.addEventListener("resize", syncViewport);
    window.visualViewport?.addEventListener("resize", syncViewport);
    return () => {
      resizeObserver.disconnect();
      window.removeEventListener("resize", syncViewport);
      window.visualViewport?.removeEventListener("resize", syncViewport);
    };
  });

  createEffect(
    () =>
      [props.agent.name, session.userId, threadResource()?.name, canChooseThreadOwnerFilter()] as const,
    ([agentName, , , canChooseOwner]) => {
      setThreadFiltersLoaded(false);
      const saved = loadConversationFilter(agentName, { query: "", ownerOnly: canChooseOwner });
      setThreadSearch(saved.query);
      setAppliedThreadSearch(saved.query);
      setOwnerThreadsOnly(canChooseOwner ? saved.ownerOnly : false);
      setThreadFiltersLoaded(true);
    },
  );

  createEffect(
    () =>
      [
        threadFiltersLoaded(),
        props.agent.name,
        appliedThreadSearch(),
        canChooseThreadOwnerFilter(),
        ownerThreadsOnly(),
      ] as const,
    ([loaded, agentName, query, canChooseOwner, ownerOnly]) => {
      if (!loaded) return;
      saveConversationFilter(agentName, { query, ownerOnly: canChooseOwner ? ownerOnly : false });
    },
  );

  const refreshThreads = async () => {
    const resource = threadResource();
    if (!resource || !canBrowse()) {
      setThreads([]);
      return;
    }
    const version = ++threadsVersion;
    setLoadingThreads(true);
    setBrowseFailure(null);
    try {
      const params = new URLSearchParams();
      params.set("limit", "100");
      if (appliedThreadSearch() && resource.search_field) {
        params.set(`${resource.search_field}~`, appliedThreadSearch());
      }
      if (canChooseThreadOwnerFilter() && ownerThreadsOnly() && resource.owner_field && session.userId) {
        params.set(resource.owner_field, session.userId);
      }
      const titleSearch = appliedThreadSearch().trim().toLowerCase();
      const rows = asRecords(
        await api(`/${encodeURIComponent(resource.name)}?${params.toString()}`, {
          org: includeAgentOrgContext(props.agent),
        }),
      )
        .map((row) => threadFrom(row, resource.owner_field))
        .filter(
          (thread) =>
            !canChooseThreadOwnerFilter() || !ownerThreadsOnly() || !session.userId || thread.ownerId === session.userId,
        )
        .filter((thread) => !titleSearch || thread.title.toLowerCase().includes(titleSearch))
        .sort(sortThreads);
      if (version === threadsVersion) setThreads(rows);
    } catch (error) {
      if (version === threadsVersion) {
        setThreads([]);
        setBrowseFailure(error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (version === threadsVersion) setLoadingThreads(false);
    }
  };

  const refreshMessages = async (threadId: string | null) => {
    const resource = messageResource();
    if (!resource || !canBrowse() || !threadId) {
      setMessages([]);
      return;
    }
    const version = ++messagesVersion;
    setLoadingMessages(true);
    setBrowseFailure(null);
    try {
      const rows = asRecords(
        await api(
          `/${encodeURIComponent(resource.name)}?thread_id=${encodeURIComponent(threadId)}&limit=500`,
          { org: includeAgentOrgContext(props.agent) },
        ),
      )
        .reverse()
        .map(messageFrom);
      if (version === messagesVersion) setMessages(rows);
    } catch (error) {
      if (version === messagesVersion) {
        setMessages([]);
        setBrowseFailure(error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (version === messagesVersion) setLoadingMessages(false);
    }
  };

  createEffect(
    () =>
      [
        props.agent.name,
        session.organizationId,
        session.userId,
        appliedThreadSearch(),
        canChooseThreadOwnerFilter(),
        ownerThreadsOnly(),
        props.threadId,
      ] as const,
    ([, , , , , , threadId]) => {
      if (threadId) setEphemeralThreadId(null);
      else setMessages([]);
      void refreshThreads();
    },
  );

  createEffect(
    () => [props.agent.name, session.organizationId, activeThreadId()] as const,
    ([, , threadId]) => {
      if (!threadId) {
        setMessages([]);
        return;
      }
      void refreshMessages(threadId);
    },
  );

  createEffect(
    () => [props.agent.name, props.threadId] as const,
    ([, threadId]) => {
      if (!threadId) {
        setEphemeralThreadId(null);
        setScratchMessages([]);
      }
    },
  );

  createEffect(
    () => props.threadId,
    (threadId) => {
      if (threadId) setComposerOpen(false);
    },
  );

  createEffect(
    () => [compactComposer(), draft()] as const,
    () => {
      queueMicrotask(syncComposerLayout);
    },
  );

  createEffect(lastMessageKey, () => {
    snapToBottom();
  });

  createEffect(
    () => [loadingMessages(), activeThreadId(), visibleMessages().length] as const,
    ([loading, threadId, count]) => {
      if (loading || !threadId || !count) return;
      snapToBottom();
    },
  );

  const startNew = () => {
    setDraft("");
    setEphemeralThreadId(null);
    setScratchMessages([]);
    setMessages([]);
    setComposerOpen(true);
    navigate({ kind: "agent", name: props.agent.name });
    queueMicrotask(resizeComposer);
  };
  const changeOwnerThreadFilter = (value: string) => setOwnerThreadsOnly(value === "mine");
  const runThreadSearch = () => setAppliedThreadSearch(threadSearch().trim());

  const openThread = (threadId: string) => {
    setComposerOpen(false);
    navigate({ kind: "agent", name: props.agent.name, threadId });
  };

  const backToList = () => {
    setComposerOpen(false);
    setDraft("");
    setEphemeralThreadId(null);
    setScratchMessages([]);
    if (props.threadId) navigate({ kind: "agent", name: props.agent.name });
  };

  const updateVisibleConversation = (next: ChatEntry[]) => {
    if (!props.agent.storage || !canBrowse()) {
      setScratchMessages(next);
      return;
    }
    if (activeThreadId()) {
      setMessages(next);
    } else {
      setScratchMessages(next);
    }
  };

  const sendMessage = async (rawMessage: string, clearDraft = false) => {
    const message = rawMessage.trim();
    if (!message || busy()) return;

    const threadId = activeThreadId();
    const nextMessages: ChatEntry[] = [
      ...visibleMessages(),
      { role: "user", content: message },
      { role: "assistant", content: "", pending: true },
    ];
    updateVisibleConversation(nextMessages);
    if (clearDraft) setDraft("");
    setBusy(true);

    let reply = "";
    let reasoning = "";
    try {
      const done = asRecord(
        await apiStream(
          `/ai/agents/${encodeURIComponent(props.agent.name)}/chat`,
          {
            method: "POST",
            org: includeAgentOrgContext(props.agent),
            body: {
              message,
              thread_id: threadId ?? undefined,
            },
          },
          (chunk) => {
            reply += chunk;
            updateVisibleConversation([
              ...nextMessages.slice(0, -1),
              { role: "assistant", content: reply, reasoning, pending: true },
            ]);
          },
          (chunk) => {
            reasoning += chunk;
            updateVisibleConversation([
              ...nextMessages.slice(0, -1),
              { role: "assistant", content: reply, reasoning, pending: true },
            ]);
          },
        ),
      );

      updateVisibleConversation([
        ...nextMessages.slice(0, -1),
        { role: "assistant", content: reply, reasoning, pending: false },
      ]);

      const nextThreadId = typeof done?.thread_id === "string" ? done.thread_id : threadId;
      if (props.agent.storage && nextThreadId) {
        if (canBrowse()) {
          if (props.threadId !== nextThreadId) {
            navigate({ kind: "agent", name: props.agent.name, threadId: nextThreadId });
          } else {
            await refreshMessages(nextThreadId);
          }
          await refreshThreads();
        } else {
          setEphemeralThreadId(nextThreadId);
        }
      }
      setComposerOpen(false);
    } catch (error) {
      updateVisibleConversation(nextMessages.slice(0, -1));
      reportError(error);
    } finally {
      setBusy(false);
      queueMicrotask(resizeComposer);
    }
  };

  const deleteConversation = async () => {
    const thread = threadToDelete();
    const resource = threadResource();
    if (!thread || !resource) return;

    setDeletingThreadId(thread.id);
    try {
      await api(`/${encodeURIComponent(resource.name)}/${encodeURIComponent(thread.id)}`, {
        method: "DELETE",
        org: includeAgentOrgContext(props.agent),
      });
      notify("success", "Conversation deleted.");
      const deletedActive = activeThreadId() === thread.id;
      if (deletedActive) {
        setComposerOpen(false);
        setDraft("");
        setEphemeralThreadId(null);
        setMessages([]);
        setScratchMessages([]);
        navigate({ kind: "agent", name: props.agent.name });
      }
      await refreshThreads();
    } catch (error) {
      reportError(error);
    } finally {
      setDeletingThreadId(null);
      setThreadToDelete(null);
    }
  };

  const conversationList = (
    <aside
      class={`flex min-h-0 min-w-0 flex-col overflow-hidden bg-surface ${
        compactLayout()
          ? "rounded-[1.125rem] border border-line"
          : "lg:h-full lg:border-r lg:border-line"
      }`}
    >
      <div class="flex shrink-0 items-start justify-between gap-4 border-b border-line p-4">
        <div>
          <p class="text-[0.6875rem] font-extrabold uppercase leading-none tracking-[0.13em] text-accent">
            Conversations
          </p>
          <h2 class="mt-1 text-base font-bold tracking-tight text-ink">History</h2>
          <p class="mt-0.5 text-xs leading-relaxed text-faint">
            {props.agent.storage
              ? appliedThreadSearch() || ownerThreadsOnly()
                ? `${threads().length ? `${threads().length} matching` : "No matching"} threads`
                : `${threads().length ? `${threads().length} stored` : "Stored"} threads`
              : "Transient chat"}
          </p>
        </div>
        <button
          type="button"
          class="inline-flex items-center gap-1.5 rounded-full border border-accent-line bg-accent px-3 py-2 text-xs font-extrabold leading-none text-on-accent transition-colors hover:bg-accent-dim"
          onClick={startNew}
        >
          <span class="text-base leading-[0.75]" aria-hidden="true">
            +
          </span>
          <span>New</span>
        </button>
      </div>
      <Show when={props.agent.storage && canBrowse()}>
        <div class="shrink-0 border-b border-line p-3">
          <div class="flex flex-wrap items-center gap-2">
            <div class="min-w-0 flex-1">
              <SearchInput
                value={threadSearch()}
                placeholder="Search conversations…"
                onInput={setThreadSearch}
                onSubmit={runThreadSearch}
              />
            </div>
            <Show when={canChooseThreadOwnerFilter()}>
              <select
                class="input w-36"
                aria-label="Conversation ownership filter"
                value={ownerThreadsOnly() ? "mine" : "everybody"}
                onChange={(event) => changeOwnerThreadFilter(event.currentTarget.value)}
              >
                <option value="everybody">Everybody</option>
                <option value="mine">Only mine</option>
              </select>
            </Show>
          </div>
        </div>
      </Show>
      <Show
        when={props.agent.storage}
        fallback={
          <div class="p-4">
            <EmptyState
              title="Transient chat"
              description="This agent answers in one live conversation only. Start a new conversation any time."
            />
          </div>
        }
      >
        <Show
          when={canBrowse()}
          fallback={
            <div class="p-4">
              <EmptyState
                title="Conversation browsing is unavailable"
                description={
                  browseFailure() ??
                  "You can still chat here, but this account cannot list the stored threads for this agent."
                }
              />
            </div>
          }
        >
          <Show
            when={threads().length}
            fallback={
              <div class="p-4">
                <EmptyState
                  title={
                    loadingThreads()
                      ? "Loading conversations…"
                      : threadFiltersApplied()
                        ? "Nothing matched those filters"
                        : "No conversations yet"
                  }
                  description={
                    threadFiltersApplied()
                      ? "Try a different search, or clear the filters to see everything."
                      : "Start a new conversation to create the first stored conversation."
                  }
                />
              </div>
            }
          >
            <div class="flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto p-3">
              <For each={threads()}>
                {(thread) => {
                  const active = () => activeThreadId() === thread.id;
                  const deleting = () => deletingThreadId() === thread.id;
                  return (
                    <div
                      class={`flex items-start gap-2 rounded-xl border p-3 transition-colors ${
                        active()
                          ? "border-accent-line bg-accent-soft"
                          : "border-transparent hover:border-line-strong hover:bg-surface-2"
                      }`}
                    >
                      <button
                        type="button"
                        class="min-w-0 flex-1 text-left"
                        aria-current={active() ? "true" : "false"}
                        onClick={() => openThread(thread.id)}
                      >
                        <div class="flex items-center justify-between gap-3">
                          <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-sm font-bold leading-snug text-ink">
                            {thread.title}
                          </span>
                          <Show when={active()}>
                            <span class="shrink-0 rounded-full border border-accent-line bg-surface px-2 py-1 text-[0.625rem] font-extrabold uppercase leading-none tracking-wide text-accent">
                              Open
                            </span>
                          </Show>
                        </div>
                        <span class="mt-1.5 block text-[0.72rem] leading-snug text-faint">
                          {formatWhen(thread.updatedAt || thread.createdAt)}
                        </span>
                      </button>
                      <Show when={canDeleteHistory()}>
                        <button
                          type="button"
                          class="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-full text-faint transition-colors hover:bg-danger-soft hover:text-danger disabled:opacity-40"
                          aria-label={`Delete ${thread.title}`}
                          disabled={deleting()}
                          onClick={() => setThreadToDelete(thread)}
                        >
                          <Show
                            when={deleting()}
                            fallback={
                              <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.6">
                                <path
                                  d="M2.75 4h8.5M5.25 2.75h3.5M5 5.75v4M7 5.75v4M9 5.75v4M3.75 4l.45 6.1a1 1 0 0 0 1 .9h3.6a1 1 0 0 0 1-.9L10.25 4"
                                  stroke-linecap="round"
                                  stroke-linejoin="round"
                                />
                              </svg>
                            }
                          >
                            <Spinner class="h-3.5 w-3.5" />
                          </Show>
                        </button>
                      </Show>
                    </div>
                  );
                }}
              </For>
            </div>
          </Show>
        </Show>
      </Show>
    </aside>
  );

  const emptyPanel = (
    <section
      class={`relative flex min-h-0 min-w-0 flex-col overflow-hidden bg-surface ${
        compactComposer() ? "h-[calc(100dvh-4rem)] min-h-[calc(100dvh-4rem)]" : "lg:h-full"
      }`}
    >
      <div class="flex flex-1 items-center justify-center p-5">
        <div class="w-full max-w-lg rounded-2xl border border-line bg-surface p-8 text-center">
          <div class="mb-4 inline-flex">
            <Avatar name={props.agent.label} />
          </div>
          <h2 class="text-xl font-extrabold tracking-tight text-ink">
            {props.agent.storage ? "Pick a conversation" : "Start a conversation"}
          </h2>
          <p class="mx-auto mb-5 mt-2 max-w-sm text-sm leading-relaxed text-muted">
            {props.agent.storage
              ? "Choose a thread from the sidebar or start a fresh chat."
              : "Open a new conversation to start chatting with this agent."}
          </p>
          <Button variant="primary" onClick={startNew}>
            New conversation
          </Button>
        </div>
      </div>
    </section>
  );

  const chatPanel = (
    <section
      class={`relative flex min-h-0 min-w-0 flex-col overflow-hidden bg-surface ${
        compactComposer() ? "h-[calc(100dvh-4rem)] min-h-[calc(100dvh-4rem)]" : "lg:h-full"
      }`}
    >
      <div
        class={`flex min-h-17 shrink-0 items-center justify-between gap-4 border-b border-line px-4 py-3 ${
          compactComposer()
            ? "fixed inset-x-0 top-15 z-30 bg-surface/80 backdrop-blur-md"
            : "bg-surface"
        }`}
      >
        <button
          type="button"
          class={`items-center gap-1.5 rounded-full border border-line px-2.5 py-1.5 text-xs font-bold text-muted ${
            compactLayout() ? "inline-flex" : "hidden"
          }`}
          onClick={backToList}
        >
          <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.9" aria-hidden="true">
            <path d="M8.75 3.5 5.25 7l3.5 3.5M5.5 7h5" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
          <span>Back</span>
        </button>
        <div class="flex min-w-0 items-center gap-3">
          <div class="shrink-0">
            <Avatar name={props.agent.label} />
          </div>
          <div class="min-w-0">
            <h2 class="overflow-hidden text-ellipsis whitespace-nowrap text-base font-extrabold leading-tight tracking-tight text-ink">
              {activeThread()?.title || (activeThreadId() ? "Conversation" : "New conversation")}
            </h2>
            <p class="mt-1 overflow-hidden text-ellipsis whitespace-nowrap text-xs leading-snug text-faint">
              {activeThread()
                ? formatWhen(activeThread()!.updatedAt || activeThread()!.createdAt)
                : "Replies stream in live as the assistant answers."}
            </p>
          </div>
        </div>
        <div class="ml-auto flex flex-wrap items-center justify-end gap-2">
          <Show when={loadingMessages()}>
            <Badge tone="neutral">Loading…</Badge>
          </Show>
          <Show when={props.agent.storage}>
            <Badge tone="success">Stored</Badge>
          </Show>
          <Show when={activeThread() && canDeleteHistory()}>
            <Button
              variant="danger"
              size="sm"
              loading={deletingThreadId() === activeThread()!.id}
              onClick={() => setThreadToDelete(activeThread())}
            >
              Delete
            </Button>
          </Show>
        </div>
      </div>

      <div
        ref={messageListRef}
        onScroll={syncScrollAffordance}
        class={`flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-4 ${
          compactComposer() ? "pt-24" : "pt-5 pb-5 sm:px-6 lg:px-8"
        }`}
        style={compactComposer() ? { "padding-bottom": `${composerHeight() + 24}px` } : undefined}
      >
        <Show
          when={visibleMessages().length}
          fallback={
            <div class="flex min-h-full flex-1 items-center justify-center">
              <EmptyState
                title={loadingMessages() ? "Loading messages…" : "No messages yet"}
                description="Ask the agent something to start the conversation."
              />
            </div>
          }
        >
          <For each={conversationBlocks()}>
            {(block) => (
              <Show
                when={block.kind === "tools"}
                fallback={
                  <div
                    class={
                      block.kind === "entry" && block.entry.role === "user"
                        ? "flex justify-end"
                        : "grid grid-cols-[2rem_minmax(0,1fr)] items-start gap-3"
                    }
                  >
                    <Show when={block.kind === "entry" && block.entry.role !== "user"}>
                      <div class="pt-0.5">
                        <Avatar name={roleLabel((block as { entry: ChatEntry }).entry.role, props.agent.label)} />
                      </div>
                    </Show>
                    {(() => {
                      const entryBlock = block as { kind: "entry"; entry: ChatEntry; index: number };
                      const entry = entryBlock.entry;
                      const author = roleLabel(entry.role, props.agent.label);
                      const userBubble = entry.role === "user";
                      const reasoningKey = `${entryBlock.index}:${entry.role}`;
                      // The toggle follows the message: a reply the model thought about has
                      // one, a reply it did not has nothing to reveal.
                      const hasReasoning = Boolean(!userBubble && entry.reasoning?.trim());
                      const selectionClass = userBubble
                        ? "selection:bg-white/90 selection:text-accent-dim"
                        : "selection:bg-accent/35 selection:text-ink";
                      return (
                        <div
                          class={`rounded-2xl border p-4 ${selectionClass} ${
                            userBubble
                              ? "w-fit max-w-[min(78%,44rem)] rounded-br-md border-accent-line bg-accent text-on-accent"
                              : entry.role === "system"
                                ? "w-[min(100%,54rem)] rounded-tl-md border-line bg-surface-2 text-ink"
                                : "w-[min(100%,54rem)] rounded-tl-md border-line bg-surface text-ink"
                          }`}
                        >
                          <div
                            class={`mb-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[0.72rem] leading-snug ${
                              userBubble ? "text-on-accent/80" : "text-faint"
                            }`}
                          >
                            <span class={`text-[0.8125rem] font-extrabold ${userBubble ? "text-on-accent" : "text-ink"}`}>
                              {author}
                            </span>
                            <Show when={entry.pending || entry.meta}>
                              <span>{entry.pending ? "Thinking…" : entry.meta}</span>
                            </Show>
                          </div>
                          <Show when={hasReasoning && expandedReasoning()[reasoningKey]}>
                            <pre class="mb-3 overflow-x-auto whitespace-pre-wrap break-words rounded-xl border border-line bg-surface-2 px-3 py-2 text-[0.75rem] leading-6 text-ink">
                              {entry.reasoning}
                            </pre>
                          </Show>
                          <div class={`text-[0.9375rem] leading-[1.72] ${userBubble ? "text-on-accent" : "text-ink"}`}>
                            <Show
                              when={!userBubble}
                              fallback={<p class="whitespace-pre-wrap break-words">{entry.content || "…"}</p>}
                            >
                              <MarkupView value={entry.content || "…"} format="markdown" />
                            </Show>
                          </div>
                          <div
                            class={`mt-3 flex items-center justify-end gap-3 text-[0.72rem] ${
                              userBubble ? "text-on-accent/80" : "text-faint"
                            }`}
                          >
                            <Show when={hasReasoning}>
                              <button
                                type="button"
                                class={`transition-colors hover:underline ${userBubble ? "hover:text-on-accent" : "hover:text-ink"}`}
                                onClick={() => toggleReasoning(reasoningKey)}
                              >
                                {expandedReasoning()[reasoningKey] ? "Hide reasoning" : "Show reasoning"}
                              </button>
                            </Show>
                            <Show when={entryBlock.index === resendableIndex()}>
                              <button
                                type="button"
                                class={`transition-colors hover:underline ${userBubble ? "hover:text-on-accent" : "hover:text-ink"}`}
                                onClick={() => void sendMessage(entry.content)}
                              >
                                Resend
                              </button>
                            </Show>
                            <button
                              type="button"
                              class={`transition-colors hover:underline ${userBubble ? "hover:text-on-accent" : "hover:text-ink"}`}
                              onClick={() => void copyText(entry.content)}
                            >
                              Copy
                            </button>
                          </div>
                        </div>
                      );
                    })()}
                  </div>
                }
              >
                <div class="grid grid-cols-[2rem_minmax(0,1fr)] items-start gap-3">
                  <div class="pt-0.5">
                    <Avatar name="Tools" />
                  </div>
                  <div class="w-[min(100%,40rem)] rounded-2xl rounded-tl-md border border-warn-line bg-warn-soft p-3 text-ink selection:bg-accent/35 selection:text-ink">
                    <button
                      type="button"
                      class="flex w-full items-center justify-between gap-3 text-left"
                      onClick={() => toggleToolGroup((block as { key: string }).key)}
                    >
                      <div class="flex min-w-0 items-center gap-2">
                        <span class="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-full border border-warn-line bg-surface text-warn">
                          <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
                            <path
                              d="M5.5 3.5h5m-5 4h5m-5 4h2M3.5 2.75h9a.75.75 0 0 1 .75.75v9a.75.75 0 0 1-.75.75h-9a.75.75 0 0 1-.75-.75v-9a.75.75 0 0 1 .75-.75Z"
                              stroke-linecap="round"
                              stroke-linejoin="round"
                            />
                          </svg>
                        </span>
                        <div class="min-w-0">
                          <div class="text-sm font-bold text-ink">Calling tools</div>
                          <div class="text-[0.72rem] leading-snug text-faint">{toolSummary((block as { entries: ChatEntry[] }).entries)}</div>
                        </div>
                      </div>
                      <svg
                        class={`h-4 w-4 shrink-0 text-faint transition-transform ${
                          expandedToolGroups()[(block as { key: string }).key] ? "rotate-180" : ""
                        }`}
                        viewBox="0 0 16 16"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="1.6"
                      >
                        <path d="m3.5 6 4.5 4.5L12.5 6" stroke-linecap="round" stroke-linejoin="round" />
                      </svg>
                    </button>

                    <Show when={expandedToolGroups()[(block as { key: string }).key]}>
                      <div class="mt-3 space-y-3 border-t border-warn-line/70 pt-3">
                        <For each={(block as { executions: ToolExecution[] }).executions}>
                          {(execution) => (
                            <div class="rounded-xl border border-warn-line/70 bg-surface/70 p-3">
                              <div class="flex flex-wrap items-center gap-2 text-[0.72rem] leading-snug text-faint">
                                <span class="text-[0.8125rem] font-extrabold text-ink">
                                  {execution.call?.toolName || execution.result?.toolName || "Tool"}
                                </span>
                                <Show when={execution.call?.toolCallId || execution.result?.toolCallId}>
                                  <span>call: {execution.call?.toolCallId || execution.result?.toolCallId}</span>
                                </Show>
                              </div>
                              <Show when={execution.call}>
                                <div class="mt-3">
                                  <p class="mb-1 text-[0.6875rem] font-extrabold uppercase tracking-[0.13em] text-faint">
                                    Call
                                  </p>
                                  <ToolPayload
                                    value={execution.call!.toolInput}
                                    fallback={execution.call!.content}
                                    onCopy={() => void copyText(displayToolPayload(execution.call!.toolInput, execution.call!.content))}
                                  />
                                </div>
                              </Show>
                              <Show when={execution.result}>
                                <div class="mt-3">
                                  <p class="mb-1 text-[0.6875rem] font-extrabold uppercase tracking-[0.13em] text-faint">
                                    Result
                                  </p>
                                  <ToolPayload
                                    value={execution.result!.toolOutput}
                                    fallback={execution.result!.content}
                                    onCopy={() => void copyText(displayToolPayload(execution.result!.toolOutput, execution.result!.content))}
                                  />
                                </div>
                              </Show>
                            </div>
                          )}
                        </For>
                      </div>
                    </Show>

                  </div>
                </div>
              </Show>
            )}
          </For>
        </Show>
      </div>

      <Show when={awayFromBottom()}>
        <button
          type="button"
          class={`inline-flex h-11 w-11 -translate-x-1/2 items-center justify-center rounded-full shadow-lg backdrop-blur-md transition-colors ${
            compactComposer()
              ? "fixed left-1/2 z-50 border border-line bg-surface/85 text-muted hover:border-line-strong hover:bg-surface/95 hover:text-ink"
              : "absolute left-1/2 z-10 border border-line bg-surface/75 text-muted hover:border-line-strong hover:bg-surface/90 hover:text-ink"
          }`}
          style={compactComposer() ? { bottom: `${composerHeight() + 16}px` } : { bottom: `${composerHeight() + 20}px` }}
          aria-label="Scroll to latest message"
          onClick={() => scrollToBottom("smooth")}
        >
          <svg width="18" height="18" viewBox="0 0 18 18" fill="none" stroke="currentColor" stroke-width="1.8">
            <path d="M9 4.5v9M9 13.5 5.5 10M9 13.5l3.5-3.5" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
        </button>
      </Show>

      <div
        ref={composerShellRef}
        class={
          compactComposer()
            ? "fixed inset-x-0 bottom-0 z-40 border-t border-line bg-surface/96 px-3 pb-[calc(0.75rem+env(safe-area-inset-bottom))] pt-3 backdrop-blur-md"
            : "shrink-0 border-t border-line bg-surface px-4 py-4 sm:px-6 lg:px-8"
        }
      >
        <div class="relative rounded-[1.75rem] border border-line-strong bg-surface shadow-lg shadow-black/10 transition-colors focus-within:border-accent focus-within:ring-3 focus-within:ring-accent/15">
          <textarea
            ref={composerRef}
            rows={2}
            class={`block w-full resize-none border-0 bg-transparent px-4 pb-4 pr-14 pt-3 text-[0.9375rem] leading-6 text-ink outline-none placeholder:text-faint ${
              compactComposer() ? "max-h-40 min-h-20" : "max-h-60 min-h-20"
            }`}
            value={draft()}
            disabled={busy()}
            onInput={(event) => {
              setDraft(event.currentTarget.value);
              resizeComposer();
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                void sendMessage(draft(), true);
              }
            }}
            placeholder={`Message ${props.agent.label}…`}
          />
          <div class="pointer-events-none absolute bottom-3 right-3">
            <button
              type="button"
              class="pointer-events-auto inline-flex h-10 w-10 items-center justify-center rounded-full bg-accent text-on-accent transition-colors hover:bg-accent-dim disabled:pointer-events-none disabled:opacity-40"
              aria-label="Send message"
              disabled={!canSend()}
              onClick={() => void sendMessage(draft(), true)}
            >
              <Show
                when={busy()}
                fallback={
                  <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8">
                    <path d="M8 13V3M8 3 4.5 6.5M8 3l3.5 3.5" stroke-linecap="round" stroke-linejoin="round" />
                  </svg>
                }
              >
                <Spinner class="h-4 w-4" />
              </Show>
            </button>
          </div>
        </div>
      </div>
    </section>
  );

  return (
    <>
      <div
        class={`flex min-h-0 flex-col lg:h-full ${
          !compactLayout() || !chatOpen() ? "gap-3" : ""
        } ${compactLayout() && !chatOpen() ? "px-4 pt-4" : ""}`}
      >
        <Show when={!compactLayout() || !chatOpen()}>
          <header class="flex shrink-0 items-end justify-between gap-4 max-lg:flex-col max-lg:items-start">
            <div>
              <p class="text-[0.6875rem] font-extrabold uppercase leading-none tracking-[0.13em] text-accent">
                AI agent
              </p>
              <h1 class="mt-1 text-[clamp(1.375rem,2vw,1.875rem)] font-bold leading-tight tracking-tight text-ink">
                {props.agent.label}
              </h1>
              <p class="mt-1 max-w-3xl text-sm leading-relaxed text-muted">
                {props.agent.description || "Chat with this configured agent."}
              </p>
            </div>
            <div class="flex flex-wrap items-center justify-end gap-2 max-lg:justify-start">
              <Badge tone={props.agent.chat.value === "public" ? "neutral" : "accent"}>{props.agent.chat.note}</Badge>
              <Show when={props.agent.storage}>
                <Badge tone="success">Stored history</Badge>
              </Show>
            </div>
          </header>
        </Show>

        <div class="min-h-0 flex-1">
          <Show
            when={compactLayout()}
            fallback={
              <div class="h-full min-h-0 overflow-hidden rounded-[1.125rem] border border-line bg-surface lg:grid lg:grid-cols-[22rem_minmax(0,1fr)]">
                {conversationList}
                <Show when={chatOpen()} fallback={emptyPanel}>
                  {chatPanel}
                </Show>
              </div>
            }
          >
            <Show when={chatOpen()} fallback={conversationList}>
              {chatPanel}
            </Show>
          </Show>
        </div>
      </div>

      <ConfirmDialog
        open={Boolean(threadToDelete())}
        title="Delete this conversation?"
        description={`This removes “${threadToDelete()?.title || "this conversation"}” from stored history.`}
        confirmLabel="Delete"
        danger
        busy={Boolean(threadToDelete()) && deletingThreadId() === threadToDelete()!.id}
        onConfirm={() => void deleteConversation()}
        onCancel={() => !deletingThreadId() && setThreadToDelete(null)}
      />
    </>
  );
}
