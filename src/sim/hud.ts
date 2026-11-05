/**
 * Optional control strip for sim.html (`?hud`): the simulated time, the
 * menu-bar title the Rust tray would show, and speed / time-travel buttons.
 * Plain DOM, styled from sim.css (no inline <style>, so the production CSP's
 * `style-src 'self'` holds).
 */
import type { SimBackend } from "./backend";
import type { SimClock } from "./clock";

type HudSim = {
  clock: SimClock;
  backend: SimBackend;
  scenario: string;
  scenarios: string[];
  advance: (amount: string | number) => string;
  surpriseReset: (id: string) => void;
  openSettings: () => void;
  showOnboarding: () => void;
};

function button(label: string, onClick: () => void, title?: string): HTMLButtonElement {
  const element = document.createElement("button");
  element.type = "button";
  element.textContent = label;
  if (title) element.title = title;
  element.addEventListener("click", onClick);
  return element;
}

export function mountHud(sim: HudSim): void {
  const hud = document.createElement("div");
  hud.className = "sim-hud";
  hud.setAttribute("data-sim-hud", "");

  const bar = document.createElement("div");
  bar.className = "sim-hud-bar";
  const tray = document.createElement("span");
  tray.className = "sim-hud-tray";
  const time = document.createElement("span");
  time.className = "sim-hud-time";
  const toggle = button("⚙", () => hud.classList.toggle("open"), "Simulator controls");
  bar.append(tray, time, toggle);

  const panel = document.createElement("div");
  panel.className = "sim-hud-panel";
  const speeds = document.createElement("div");
  speeds.className = "sim-hud-row";
  speeds.append("Speed ");
  for (const speed of [0, 1, 60, 600, 3600]) {
    speeds.append(button(speed === 0 ? "❚❚" : `${speed}×`, () => sim.clock.setSpeed(speed)));
  }
  const jumps = document.createElement("div");
  jumps.className = "sim-hud-row";
  jumps.append("Jump ");
  for (const amount of ["10m", "1h", "6h", "1d"]) jumps.append(button(`+${amount}`, () => sim.advance(amount)));
  const actions = document.createElement("div");
  actions.className = "sim-hud-row";
  actions.append(
    button("Surprise reset", () => sim.surpriseReset("codex"), "Renew Codex's windows now"),
    button("Settings", () => sim.openSettings()),
    button("Guide", () => sim.showOnboarding()),
  );
  const scenarios = document.createElement("div");
  scenarios.className = "sim-hud-row";
  const select = document.createElement("select");
  for (const id of sim.scenarios) {
    const option = document.createElement("option");
    option.value = id;
    option.textContent = id;
    option.selected = id === sim.scenario;
    select.append(option);
  }
  select.addEventListener("change", () => {
    const next = new URLSearchParams(location.search);
    next.set("scenario", select.value);
    location.search = next.toString();
  });
  scenarios.append("Scenario ", select);
  panel.append(speeds, jumps, actions, scenarios);
  hud.append(bar, panel);
  document.body.append(hud);

  const render = () => {
    const now = new Date(sim.clock.now());
    time.textContent = `${now.toISOString().slice(5, 16).replace("T", " ")}Z · ${sim.clock.speed}×`;
    tray.textContent = sim.backend.trayTitles().title || "—";
  };
  render();
  window.setInterval(render, 500);
}
