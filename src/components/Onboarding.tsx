import { useEffect, useState } from "react";
import {
  isPermissionGranted,
  requestPermission,
} from "@tauri-apps/plugin-notification";
import {
  BellRing,
  Check,
  ChevronLeft,
  ChevronRight,
  KeyRound,
  MousePointerClick,
  Radio,
  Sparkles,
} from "lucide-react";
import { MeterMark } from "./MeterMark";

type ProviderStatus = {
  label: string;
  connected: boolean;
  detail: string;
  onRetry: () => void;
};

type Props = {
  codex: ProviderStatus;
  claude: ProviderStatus | null;
  onClose: () => void;
};

const STEP_COUNT = 4;

function StepWelcome() {
  return (
    <>
      <div className="onboard-hero" aria-hidden="true">
        <MeterMark />
        <span className="onboard-hero-sample">
          <strong>63%</strong> · 4:12:08
        </span>
      </div>
      <h2>Your quota, always visible</h2>
      <p>
        UsageBar puts one live meter per provider in your menu bar, so you never
        open a terminal to find out how much is left.
      </p>
      <ul className="onboard-list">
        <li>
          <span className="onboard-bullet">%</span>
          <span>
            Percentages are <strong>how much you have used</strong> — the same
            way Codex and Claude Code report them.
          </span>
        </li>
        <li>
          <span className="onboard-bullet">
            <MousePointerClick size={13} aria-hidden="true" />
          </span>
          <span>Click a menu-bar icon to open this panel. Right-click it for settings.</span>
        </li>
      </ul>
    </>
  );
}

function StepProviders({ codex, claude }: { codex: ProviderStatus; claude: ProviderStatus | null }) {
  return (
    <>
      <h2>Connections</h2>
      <p>
        Nothing to configure — UsageBar reads the logins these tools already
        keep on this Mac.
      </p>
      <div className="onboard-providers">
        {[codex, claude].filter((value): value is ProviderStatus => value !== null).map((provider) => (
          <div key={provider.label} className={`onboard-provider${provider.connected ? " is-ready" : ""}`}>
            <span className="onboard-provider-dot" aria-hidden="true" />
            <div>
              <strong>{provider.label}</strong>
              <span>{provider.detail}</span>
            </div>
            {provider.connected ? (
              <Check size={16} aria-hidden="true" />
            ) : (
              <button className="secondary-button" onClick={provider.onRetry}>
                Retry
              </button>
            )}
          </div>
        ))}
      </div>
      <div className="onboard-note">
        <KeyRound size={14} aria-hidden="true" />
        <p>
          For Claude Code, macOS asks once for Keychain access. Choose{" "}
          <strong>Always Allow</strong> so background refresh stays silent.
          UsageBar only ever reads that token and sends it to Anthropic's own
          usage endpoint — never anywhere else.
        </p>
      </div>
    </>
  );
}

function StepAlerts() {
  const [granted, setGranted] = useState<boolean | null>(null);
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    void isPermissionGranted()
      .then(setGranted)
      .catch(() => setGranted(null));
  }, []);

  const enable = async () => {
    try {
      setGranted((await requestPermission()) === "granted");
    } catch {
      setGranted(false);
    }
  };

  return (
    <>
      <h2>Never miss a reset</h2>
      <p>
        UsageBar watches for surprise quota resets and tells you{" "}
        <strong>before</strong> they land — so you can spend what is left of the
        current window instead of saving it for nothing.
      </p>
      <ul className="onboard-list">
        <li>
          <span className="onboard-bullet">
            <Radio size={13} aria-hidden="true" />
          </span>
          <span>
            Reset radar tracks public announcements and flags an incoming reset
            with a ⚡ in the menu bar.
          </span>
        </li>
        <li>
          <span className="onboard-bullet">
            <BellRing size={13} aria-hidden="true" />
          </span>
          <span>
            Usage alerts fire when a window passes 80% and 95% used, and when a
            fresh window starts.
          </span>
        </li>
      </ul>
      {granted ? (
        <div className="onboard-note is-ready">
          <Check size={14} aria-hidden="true" />
          <p>Notifications are enabled.</p>
        </div>
      ) : (
        <button className="primary-button" onClick={() => void enable()}>
          Enable notifications
        </button>
      )}
    </>
  );
}

function StepSettings() {
  return (
    <>
      <h2>Make it yours</h2>
      <p>Right-click either menu-bar icon to find:</p>
      <ul className="onboard-list">
        <li>
          <span className="onboard-bullet">
            <Sparkles size={13} aria-hidden="true" />
          </span>
          <span>
            <strong>Menu Bar Shows</strong> — pick which window the meter
            follows: most used, 5-hour, weekly, or a per-model limit like Fable.
          </span>
        </li>
        <li>
          <span className="onboard-bullet">%</span>
          <span>
            <strong>Compact Meter</strong> — percentage only, no countdown, for
            tight menu bars.
          </span>
        </li>
        <li>
          <span className="onboard-bullet">
            <Check size={13} aria-hidden="true" />
          </span>
          <span>
            <strong>Launch at Login</strong> and <strong>Usage Alerts</strong>.
          </span>
        </li>
      </ul>
      <p className="onboard-footnote">
        You can reopen this guide any time from the menu-bar icon → Setup Guide.
      </p>
    </>
  );
}

export function Onboarding({ codex, claude, onClose }: Props) {
  const [step, setStep] = useState(0);
  const last = step === STEP_COUNT - 1;

  return (
    <div className="onboard-scrim" role="dialog" aria-modal="true" aria-label="UsageBar setup">
      <div className="onboard-card glass-tile">
        <div className="onboard-body">
          <div className="onboard-content">
            {step === 0 ? <StepWelcome /> : null}
            {step === 1 ? <StepProviders codex={codex} claude={claude} /> : null}
            {step === 2 ? <StepAlerts /> : null}
            {step === 3 ? <StepSettings /> : null}
          </div>
        </div>
        <div className="onboard-footer">
          <button
            className="icon-button"
            onClick={() => setStep((value) => Math.max(0, value - 1))}
            disabled={step === 0}
            aria-label="Previous step"
          >
            <ChevronLeft size={16} />
          </button>
          <div className="onboard-dots" role="presentation">
            {Array.from({ length: STEP_COUNT }, (_, index) => (
              <span key={index} className={index === step ? "is-active" : ""} />
            ))}
          </div>
          {last ? (
            <button className="primary-button" onClick={onClose}>
              Done
            </button>
          ) : (
            <>
              <button className="ghost-button" onClick={onClose}>
                Skip
              </button>
              <button className="primary-button" onClick={() => setStep((value) => value + 1)}>
                Next <ChevronRight size={15} aria-hidden="true" />
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
