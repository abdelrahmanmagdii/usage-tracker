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
  Columns2,
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
  providers: ProviderStatus[];
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
      <h2>Welcome to UsageBar</h2>
      <p>
        It keeps your coding-tool usage in the menu bar, so you can
        check it without running anything.
      </p>
      <ul className="onboard-list">
        <li>
          <span className="onboard-bullet">%</span>
          <span>
            The number is <strong>how much you have left</strong>, the same as
            the tools themselves show you.
          </span>
        </li>
        <li>
          <span className="onboard-bullet">
            <MousePointerClick size={13} aria-hidden="true" />
          </span>
          <span>Click an icon to open this window. Right-click it for quick settings.</span>
        </li>
      </ul>
    </>
  );
}

function StepProviders({ providers }: { providers: ProviderStatus[] }) {
  return (
    <>
      <h2>Connections</h2>
      <p>
        There is nothing to set up. UsageBar uses the logins these tools already
        keep on this Mac. You can hide any of them later in Settings.
      </p>
      <div className="onboard-providers">
        {providers.map((provider) => (
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
          The first time it reads a stored login, macOS may ask for Keychain
          access. Pick <strong>Always Allow</strong> so it can keep updating in
          the background. UsageBar only reads those tokens, and only sends them
          to that tool’s own usage endpoint.
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
      <h2>Resets</h2>
      <p>
        Usage limits sometimes get reset early. UsageBar tells you when one is
        announced, while you still have time to use up what you have.
      </p>
      <ul className="onboard-list">
        <li>
          <span className="onboard-bullet">
            <Radio size={13} aria-hidden="true" />
          </span>
          <span>A bolt shows up in the menu bar when a reset is on the way.</span>
        </li>
        <li>
          <span className="onboard-bullet">
            <BellRing size={13} aria-hidden="true" />
          </span>
          <span>
            You can also be notified when a limit reaches 80% and 95%, and when
            it resets.
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
      <h2>Settings</h2>
      <p>
        Open Settings from the button at the bottom of this window, or by
        right-clicking a menu-bar icon.
      </p>
      <ul className="onboard-list">
        <li>
          <span className="onboard-bullet">
            <Sparkles size={13} aria-hidden="true" />
          </span>
          <span>
            Pick which limit each meter tracks. By default it follows whichever
            one you are closest to using up.
          </span>
        </li>
        <li>
          <span className="onboard-bullet">
            <Columns2 size={13} aria-hidden="true" />
          </span>
          <span>
            <strong>Compact</strong> puts every visible tool under one menu-bar
            icon; <strong>Extended</strong> gives each its own. Compact is less
            likely to get hidden when the bar is full.
          </span>
        </li>
        <li>
          <span className="onboard-bullet">
            <Check size={13} aria-hidden="true" />
          </span>
          <span>Turn on Launch at Login so it starts with your Mac.</span>
        </li>
      </ul>
      <p className="onboard-footnote">
        This guide stays in Settings if you want to read it again.
      </p>
    </>
  );
}

export function Onboarding({ providers, onClose }: Props) {
  const [step, setStep] = useState(0);
  const last = step === STEP_COUNT - 1;

  return (
    <div className="onboard-scrim" role="dialog" aria-modal="true" aria-label="UsageBar setup">
      <div className="onboard-card glass-tile">
        <div className="onboard-body">
          <div className="onboard-content">
            {step === 0 ? <StepWelcome /> : null}
            {step === 1 ? <StepProviders providers={providers} /> : null}
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
