import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "./styles/tokens.css";
import "./styles/global.css";
import { App } from "./App";
import { bootstrapTheme } from "./lib/theme";

// Apply the remembered theme before the first render (no flash of the wrong theme).
bootstrapTheme();

const container = document.getElementById("root");
if (container) {
  createRoot(container).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
