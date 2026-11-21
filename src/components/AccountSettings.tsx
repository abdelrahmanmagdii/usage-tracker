import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FolderPlus, RotateCcw, Trash2 } from "lucide-react";
import { PROVIDER_CATALOG, type ProviderId } from "../lib/providers";
import { isRecord } from "../lib/rateLimits";
import type { AccountSources } from "../lib/accounts";

function asSources(value: unknown): AccountSources | null {
  if (!isRecord(value) || !Array.isArray(value.choices) || typeof value.activeId !== "string") return null;
  return value as unknown as AccountSources;
}

const MAX_LABEL = 40;
const inTauri = () => "__TAURI_INTERNALS__" in window;

/**
 * Which saved login each tool reads. A change is applied by restarting
 * UsageBar, so no earlier reading or cached token can outlive the switch.
 * Nothing here changes a login or copies a secret.
 */
export function AccountSettings() {
  const [provider, setProvider] = useState<ProviderId>("codex");
  const [info, setInfo] = useState<AccountSources | null>(null);
  const [selected, setSelected] = useState("default");
  const [label, setLabel] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const apply = useCallback((next: AccountSources | null, preferred?: string) => {
    setInfo(next);
    if (next) setSelected(preferred ?? next.activeId);
  }, []);

  useEffect(() => {
    let active = true;
    setError(null);
    setInfo(null);
    if (!inTauri()) return;
    void invoke("get_account_sources", { provider })
      .then((value) => {
        if (active) apply(asSources(value));
      })
      .catch((reason) => {
        if (active) setError(String(reason));
      });
    return () => {
      active = false;
    };
  }, [provider, apply]);

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const choice = info?.choices.find((entry) => entry.id === selected);
  const changed = info !== null && selected !== info.activeId;
  const providerName = PROVIDER_CATALOG.find((tool) => tool.id === provider)?.label ?? provider;

  const add = () =>
    run(async () => {
      const next = asSources(
        await invoke("add_account_profile", { provider, label: label.trim() || null }),
      );
      if (!next) return;
      const known = new Set(info?.choices.map((entry) => entry.id));
      const added = next.choices.find((entry) => entry.custom && !known.has(entry.id));
      apply(next, added?.id ?? selected);
      setLabel("");
    });

  const forget = () =>
    run(async () => {
      apply(asSources(await invoke("forget_account_profile", { provider, id: selected })));
    });

  const switchProfile = () =>
    run(async () => {
      await invoke("switch_account_profile", { provider, id: selected });
      // The app restarts after a real switch; a backend that does not (the
      // simulator) must not leave the old "(active)" marker on screen.
      apply(asSources(await invoke("get_account_sources", { provider })));
    });

  const reset = () =>
    run(async () => {
      await invoke("reset_account_mapping", { provider });
    });

  return (
    <div className="setting-group accounts-panel">
      <span className="setting-label">Accounts</span>
      <p className="setting-hint">
        Pick the saved login each tool reads. Switching restarts UsageBar only;
        {" "}the tool's own app keeps running and its login is never changed.
      </p>
      <label className="account-field">
        <span>Tool</span>
        <select
          value={provider}
          disabled={busy}
          onChange={(event) => setProvider(event.target.value as ProviderId)}
        >
          {PROVIDER_CATALOG.map((tool) => (
            <option key={tool.id} value={tool.id}>
              {tool.label}
            </option>
          ))}
        </select>
      </label>
      {!inTauri() ? (
        <p className="setting-hint">Open the desktop app to choose a local account.</p>
      ) : null}
      {info ? (
        <>
          <label className="account-field">
            <span>Login source</span>
            <select value={selected} disabled={busy} onChange={(event) => setSelected(event.target.value)}>
              {info.choices.map((entry) => (
                <option key={entry.id} value={entry.id}>
                  {entry.label}
                  {entry.id === info.activeId ? " (active)" : ""}
                </option>
              ))}
            </select>
          </label>
          {info.unavailable ? <p className="setting-hint is-warning" role="status">{info.unavailable}</p> : null}
          {info.diagnostic ? (
            <div role="alert">
              <p className="setting-hint is-warning">{info.diagnostic}</p>
              <button className="secondary-button" disabled={busy} onClick={() => void reset()}>
                <RotateCcw size={13} aria-hidden="true" /> Reset account list and restart
              </button>
            </div>
          ) : null}
          <div className="account-actions">
            <button
              className="primary-button"
              disabled={busy || !changed || Boolean(info.diagnostic)}
              onClick={() => void switchProfile()}
            >
              Use profile and restart UsageBar
            </button>
            <button
              className="secondary-button"
              disabled={busy || !choice?.custom || selected === info.activeId}
              onClick={() => void forget()}
              title="Removes only UsageBar's reference to the folder."
            >
              <Trash2 size={13} aria-hidden="true" /> Forget profile
            </button>
          </div>
          <label className="account-field">
            <span>Profile name (optional)</span>
            <input
              type="text"
              value={label}
              maxLength={MAX_LABEL}
              placeholder="Work, Team, Personal"
              disabled={busy}
              onChange={(event) => setLabel(event.target.value)}
            />
          </label>
          <button
            className="secondary-button"
            disabled={busy || Boolean(info.diagnostic)}
            onClick={() => void add()}
          >
            <FolderPlus size={13} aria-hidden="true" /> Add local profile…
          </button>
          <p className="setting-hint">
            {providerName}: {info.folderHint} Forgetting a profile removes only
            UsageBar's reference to its folder.
          </p>
        </>
      ) : null}
      {error ? <p className="setting-hint is-warning" role="alert">{error}</p> : null}
    </div>
  );
}
