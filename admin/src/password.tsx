/**
 * Choosing a password, twice.
 *
 * Every screen where a password is *set* rather than entered from memory
 * (signing up, accepting an invitation, finishing a reset) collects it in two
 * fields and refuses to submit until they match. A typo in a masked field
 * would otherwise surface at the next sign-in.
 *
 * Not shared with the *sign-in* form, which has a single field: there is
 * nothing to confirm against a value the server already holds.
 */

import { createSignal } from "solid-js";
import { Field } from "./ui";

export interface PasswordPair {
  /** The chosen password, or `""` while the two boxes disagree. */
  value: () => string;
  /** Whether both boxes are filled in and identical. */
  ready: () => boolean;
  /** The mismatch to show under the second box, once it has been typed in. */
  error: () => string | null;
  /** Clear both fields after a successful submit. */
  reset: () => void;
  password: () => string;
  setPassword: (value: string) => void;
  confirmation: () => string;
  setConfirmation: (value: string) => void;
}

export function createPasswordPair(): PasswordPair {
  const [password, setPassword] = createSignal("");
  const [confirmation, setConfirmation] = createSignal("");

  const matches = () => Boolean(password()) && password() === confirmation();

  return {
    password,
    setPassword,
    confirmation,
    setConfirmation,
    ready: matches,
    value: () => (matches() ? password() : ""),
    // Only report a mismatch once both fields have content; an error under a
    // field still being typed into is noise.
    error: () =>
      confirmation() && password() !== confirmation() ? "These two do not match." : null,
    reset: () => {
      setPassword("");
      setConfirmation("");
    },
  };
}

export function PasswordFields(props: {
  pair: PasswordPair;
  label?: string;
  help?: string | null;
  /** `new-password` everywhere; exposed so a browser is told what this is. */
  autocomplete?: string;
}) {
  return (
    <>
      <Field label={props.label ?? "Password"} help={props.help} required>
        <input
          class="input"
          type="password"
          autocomplete={props.autocomplete ?? "new-password"}
          value={props.pair.password()}
          onInput={(event) => props.pair.setPassword(event.currentTarget.value)}
        />
      </Field>
      <Field label="Confirm password" error={props.pair.error()} required>
        <input
          class="input"
          type="password"
          autocomplete={props.autocomplete ?? "new-password"}
          value={props.pair.confirmation()}
          onInput={(event) => props.pair.setConfirmation(event.currentTarget.value)}
        />
      </Field>
    </>
  );
}
