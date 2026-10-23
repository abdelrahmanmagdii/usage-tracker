import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";
import { CodexWidget } from "./components/CodexWidget";

const widget = new URLSearchParams(window.location.search).has("widget");
if (widget) document.documentElement.dataset.widget = "codex";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {widget ? <CodexWidget /> : <App />}
  </StrictMode>,
);
