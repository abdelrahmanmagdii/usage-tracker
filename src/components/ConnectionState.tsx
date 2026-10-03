import { AlertCircle, PlugZap } from "lucide-react";
import type { CodexBackendState } from "../types/codex";

export function ConnectionStateView({
  state,
  onRetry,
  compact = false,
}: {
  state: CodexBackendState;
  onRetry: () => void;
  /** Other tools are on screen, so Codex only needs a one-line note. */
  compact?: boolean;
}) {
  if (state.connection === "cli_not_found") return null;
  if (compact && state.connection !== "starting") {
    const loggedOut = state.connection === "not_authenticated";
    return (
      <div className="provider-note glass-tile codex-note" role="status">
        <p>
          <strong>{loggedOut ? "Codex is signed out." : "Codex isn't reachable."}</strong>{" "}
          {loggedOut
            ? "Sign in with the codex CLI to see its quota."
            : "Its meter comes back once the codex CLI is installed and can start."}
        </p>
        <button className="secondary-button" onClick={onRetry}>Reconnect</button>
      </div>
    );
  }

  if (state.connection === "starting") {
    return (
      <div className="state-panel glass-tile" role="status">
        <span className="spinner" aria-hidden="true" />
        <h2>Reading your meter…</h2>
        <p>Connecting to the local Codex App Server.</p>
      </div>
    );
  }

  const loggedOut = state.connection === "not_authenticated";
  const Icon = loggedOut ? PlugZap : AlertCircle;
  const title = loggedOut ? "Codex sign-in required" : "App Server unavailable";
  const message = loggedOut
    ? "Sign in through Codex first. UsageBar uses that existing session—never an API key."
    : state.diagnostic || "UsageBar lost its local connection. Your data stays on this Mac.";

  return (
    <div className="state-panel glass-tile" role="alert">
      <Icon size={22} strokeWidth={1.8} aria-hidden="true" />
      <h2>{title}</h2>
      <p>{message}</p>
      <button className="secondary-button" onClick={onRetry}>Reconnect</button>
    </div>
  );
}
