import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import App from "./App";
import { ImeOverlay } from "./components/ImeOverlay";
import "./index.css";

const Root = getCurrentWebviewWindow().label === "ime-overlay" ? ImeOverlay : App;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
