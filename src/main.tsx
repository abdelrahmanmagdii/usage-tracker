import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";
import { CodexWidget } from "./components/CodexWidget";
import { isWindows } from "./lib/platform";

const widget = new URLSearchParams(window.location.search).has("widget");
if (widget) document.documentElement.dataset.widget = "codex";
// The Windows window is opaque with no vibrancy behind it; styles.css gives
// it a solid base in dark mode.
if (isWindows) document.documentElement.dataset.platform = "windows";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {widget ? <CodexWidget /> : <App />}
  </StrictMode>,
);
