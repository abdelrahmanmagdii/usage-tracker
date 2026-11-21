import { isRecord } from "./rateLimits";

/** Which login a reading belongs to, from the sanitized `account` object. */
export type AccountContext = {
  /** Opaque id of one account under one source. Absent when none is known. */
  scopeId?: string;
  /** Short name of the source or profile, as saved by the user. */
  sourceLabel?: string;
  /** Masked identifier such as `a***@example.com`. Never a full email. */
  maskedIdentity?: string;
};

const text = (value: unknown, max: number): string | undefined =>
  typeof value === "string" && value.length > 0 && value.length <= max ? value : undefined;

export function accountContext(account: unknown): AccountContext {
  if (!isRecord(account)) return {};
  return {
    scopeId: text(account.scopeId, 128),
    sourceLabel: text(account.sourceLabel, 80),
    maskedIdentity: text(account.maskedIdentity, 120),
  };
}

export type AccountChoice = { id: string; label: string; custom: boolean };

export type AccountSources = {
  activeId: string;
  choices: AccountChoice[];
  folderHint: string;
  /** The saved list cannot be read; resetting it is a deliberate choice. */
  diagnostic?: string | null;
  /** The active choice cannot be used right now. */
  unavailable?: string | null;
};
