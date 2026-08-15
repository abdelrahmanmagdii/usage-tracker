import { AlertCircle, Binary, PlugZap } from "lucide-react";
import type { CodexBackendState } from "../types/codex";

export function ConnectionStateView({
  state,
  onRetry,
}: {
  state: CodexBackendState;
  onRetry: () => void;
}) {
  if (state.connection === "starting") {
    return (
      <div className="state-panel glass-tile" role="status">
        <span className="spinner" aria-hidden="true" />
        <h2>Reading your meter…</h2>
        <p>Connecting to the local Codex App Server.</p>
      </div>
    );
  }

  const cliMissing = state.connection === "cli_not_found";
  const loggedOut = state.connection === "not_authenticated";
  const Icon = cliMissing ? Binary : loggedOut ? PlugZap : AlertCircle;
  const title = cliMissing
    ? "Codex CLI not found"
    : loggedOut
      ? "Codex sign-in required"
      : "App Server unavailable";
  const message = cliMissing
    ? "Install the official Codex CLI and make sure the codex command is available in your shell."
    : loggedOut
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
