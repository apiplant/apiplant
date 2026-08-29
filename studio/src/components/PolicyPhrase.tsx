/**
 * A permission written as a sentence.
 *
 * A policy is three decisions — what it does, who it names, and which class of
 * organisation it is narrowed to — and a row of three controls says all of it
 * without ever saying it out loud. Written as a phrase, the same three
 * decisions read as the rule they are: "allow for members of the active
 * organisation in organisation with any class". The parts that can be changed
 * are words inside that sentence, accent-coloured and dotted-underlined; a
 * click turns one into the control it always was, and it goes back to being a
 * word as soon as it is done. Nothing about the model is hidden — the same
 * `Subject` goes in and out — but the form now reads like the rule it writes.
 */

import { For, Show, createMemo, createSignal, createUniqueId } from "solid-js";
import { policyVocabulary, type Subject } from "../lib/permissions";
import type { Effect } from "../lib/types";

/** How each level reads inside the sentence, and in the menu that sets it. */
const WHO: Record<string, { word: string; label: string }> = {
  public: { word: "everybody", label: "everybody" },
  authenticated: {
    word: "any signed-in caller",
    label: "any signed-in caller",
  },
  member: {
    word: "members of the active organisation",
    label: "members of the active organisation",
  },
  owner: { word: "owners of the row", label: "owners of the row" },
  role: { word: "anyone with the role", label: "anyone with a named role" },
  private: { word: "no-one", label: "no-one — do not expose it at all" },
};

/**
 * Levels the organisation class cannot narrow.
 *
 * A class is a test on the organisation the caller is acting in, and neither of
 * these is acting in one: `public` is an unauthenticated request, and `no-one`
 * is the absence of a caller altogether. Trailing the sentence with "in
 * organisation with any class" there would offer a narrowing that does nothing.
 */
const UNCLASSED = ["public", "private"];

const EFFECTS: { value: Effect; word: string; label: string }[] = [
  { value: "allow", word: "allow", label: "allow" },
  {
    value: "own",
    word: "allow only if they own the row",
    label: "allow only if they own the row",
  },
  { value: "deny", word: "deny", label: "deny" },
];

/** How a level reads when it is not one this form knows — a hand-written file. */
const unknownWho = (level: string) => ({ word: level, label: level });

const whoOf = (level: string) => WHO[level] ?? unknownWho(level);

export const effectWord = (effect: Effect) =>
  EFFECTS.find((option) => option.value === effect)?.word ?? effect;

/**
 * One word in the sentence that opens a menu.
 *
 * The select replaces the word in place rather than sitting beside it, so the
 * line never reflows when it is opened, and it closes on the first choice —
 * picking a level is one decision, not a form to be dismissed.
 */
function TokenSelect(props: {
  value: string;
  word: string;
  options: readonly { value: string; label: string }[];
  title: string;
  onChange: (value: string) => void;
}) {
  const [open, setOpen] = createSignal(false);
  return (
    <Show
      when={open()}
      fallback={
        <button
          type="button"
          class="token"
          title={props.title}
          onClick={() => setOpen(true)}
        >
          {props.word}
        </button>
      }
    >
      <select
        class="input token-editor"
        value={props.value}
        ref={(element) =>
          queueMicrotask(() => {
            element.focus();
            // Opens the menu on the click that revealed the select, so the
            // word behaves like the dropdown it stands for — one click, not
            // two. Not every browser has it, and it throws where the gesture
            // has already been spent; the focused select is the fallback.
            try {
              (element as HTMLSelectElement & { showPicker?: () => void })
                .showPicker?.();
            } catch {
              /* the select is focused either way */
            }
          })
        }
        onChange={(event) => {
          props.onChange(event.currentTarget.value);
          setOpen(false);
        }}
        onBlur={() => setOpen(false)}
      >
        <For each={props.options}>
          {(option) => <option value={option.value}>{option.label}</option>}
        </For>
      </select>
    </Show>
  );
}

/**
 * One word in the sentence that is typed rather than chosen.
 *
 * Roles and classes are declared nowhere — they are membership and
 * organisation data — so the input is free text with the project's own
 * vocabulary offered as completions. It is sized to what it holds so the
 * sentence keeps its shape, and commits on blur or Enter: a keystroke-by-
 * keystroke write would rewrite the policy string under the caret.
 */
function TokenText(props: {
  value: string;
  placeholder: string;
  list?: string;
  title: string;
  onCommit: (value: string) => void;
}) {
  const [draft, setDraft] = createSignal<string | null>(null);

  const commit = () => {
    const next = draft();
    setDraft(null);
    if (next !== null && next.trim() !== props.value) props.onCommit(next.trim());
  };

  return (
    <Show
      when={draft() !== null}
      fallback={
        <button
          type="button"
          class={`token ${props.value ? "" : "token-empty"}`}
          title={props.title}
          onClick={() => setDraft(props.value)}
        >
          {props.value || props.placeholder}
        </button>
      }
    >
      <input
        class="input token-editor font-mono text-[0.78125rem]"
        // Wide enough for what it holds and then some: `ch` is the width of a
        // zero, and the padding and border are outside the text, so a box cut
        // to the exact character count hides the last letter of what was just
        // typed and asks to be scrolled. It grows with the draft, and never
        // starts smaller than a short word.
        style={{
          width: `${Math.max((draft() ?? "").length, props.placeholder.length, 6) + 5}ch`,
        }}
        value={draft() ?? ""}
        placeholder={props.placeholder}
        list={props.list}
        spellcheck={false}
        autocomplete="off"
        autocapitalize="none"
        autocorrect="off"
        ref={(element) => queueMicrotask(() => element.select())}
        onInput={(event) => {
          const value = event.currentTarget.value.toLowerCase();
          if (event.currentTarget.value !== value)
            event.currentTarget.value = value;
          setDraft(value);
        }}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") event.currentTarget.blur();
          if (event.key === "Escape") {
            setDraft(null);
            event.currentTarget.blur();
          }
        }}
      />
    </Show>
  );
}

/** The role and class names the project already spells, as completion lists. */
function VocabularyLists(props: { roles: string; classes: string }) {
  const vocabulary = createMemo(() => policyVocabulary());
  return (
    <>
      <datalist id={props.roles}>
        <For each={vocabulary().roles}>{(role) => <option value={role} />}</For>
      </datalist>
      <datalist id={props.classes}>
        <For each={vocabulary().classes}>
          {(name) => <option value={name} />}
        </For>
      </datalist>
    </>
  );
}

export function PolicyPhrase(props: {
  subject: Subject;
  onChange: (update: (subject: Subject) => Subject) => void;
  /** Levels this policy may name, in menu order. */
  levels: readonly string[];
  /**
   * What the clause does. Omitted where the setting holds a single policy and
   * "allow" is the only thing it could mean — a global admin role is not
   * something you deny.
   */
  effect?: Effect;
  onEffectChange?: (effect: Effect) => void;
  /** Effects the clause may take; defaults to all three. */
  effects?: readonly Effect[];
  /** Off where the policy is not read inside an organisation at all. */
  showClass?: boolean;
  class?: string;
}) {
  const id = createUniqueId();
  const roleList = `policy-roles-${id}`;
  const classList = `policy-classes-${id}`;

  const levelOptions = createMemo(() => {
    const options = props.levels.map((level) => ({
      value: level,
      label: whoOf(level).label,
    }));
    // A level the file already names but this form would not offer — `owner`
    // in a column that has moved on, or something hand-written — is still the
    // value in the box, so it has to be in the menu or the menu lies.
    return options.some((option) => option.value === props.subject.level)
      ? options
      : [
          ...options,
          { value: props.subject.level, label: whoOf(props.subject.level).label },
        ];
  });

  const effectOptions = createMemo(() => {
    const offered = props.effects ?? ["allow", "own", "deny"];
    return EFFECTS.filter(
      (option) =>
        offered.includes(option.value) ||
        // An effect the file already uses but this form would not offer (an
        // `own` clause on create, say) stays in the menu — otherwise the box
        // would show a value that is not one of its options.
        option.value === props.effect,
    ).map((option) => ({ value: option.value, label: option.label }));
  });

  const setLevel = (level: string) =>
    props.onChange((subject) => ({
      ...subject,
      level,
      // Choosing "anyone with the role" with no role names nobody, so the
      // sentence starts from the role most projects have.
      role: level === "role" ? subject.role || "admin" : "",
    }));

  /**
   * Whether the class tail says anything here — see UNCLASSED. Off entirely
   * where the policy is not read inside an organisation at all.
   */
  const showsClass = () =>
    props.showClass !== false && !UNCLASSED.includes(props.subject.level);

  return (
    <span class={`phrase ${props.class ?? ""}`}>
      <VocabularyLists roles={roleList} classes={classList} />

      <Show when={props.effect}>
        {(effect) => (
          <>
            <TokenSelect
              value={effect()}
              word={effectWord(effect())}
              options={effectOptions()}
              title="What this clause does"
              onChange={(value) => props.onEffectChange?.(value as Effect)}
            />{" "}
            for{" "}
          </>
        )}
      </Show>

      <TokenSelect
        value={props.subject.level}
        word={whoOf(props.subject.level).word}
        options={levelOptions()}
        title="Who this names"
        onChange={setLevel}
      />

      <Show when={props.subject.level === "role"}>
        {" "}
        <TokenText
          value={props.subject.role}
          placeholder="admin"
          list={roleList}
          title="The role name, as it is spelled in membership data"
          onCommit={(role) => props.onChange((subject) => ({ ...subject, role }))}
        />
      </Show>

      {/* Parenthesised: the narrowing is an aside most clauses never use, and
          the sentence should still read as a sentence with it skipped. */}
      <Show when={showsClass()}>
        {" "}
        (in organisation with{" "}
        <TokenText
          value={props.subject.orgClass}
          placeholder="any"
          list={classList}
          title="Narrow this to organisations of one class; leave it empty for every organisation"
          onCommit={(orgClass) =>
            props.onChange((subject) => ({ ...subject, orgClass }))
          }
        />{" "}
        class)
      </Show>
    </span>
  );
}
