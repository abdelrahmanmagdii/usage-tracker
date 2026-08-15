import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { NotchSurface } from "./notch/NotchSurface";
import "./styles.css";

const isNotch = new URLSearchParams(window.location.search).get("view") === "notch";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {isNotch ? <NotchSurface /> : <App />}
  </StrictMode>,
);
