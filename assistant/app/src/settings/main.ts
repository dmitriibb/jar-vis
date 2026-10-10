import { mount } from "svelte";
import "../lib/base.css";
import Settings from "./Settings.svelte";

// A plain browser has no Tauri backend; dev builds fall back to a mock (see lib/mock.ts).
if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
  (await import("../lib/mock")).install();
}

export default mount(Settings, { target: document.getElementById("app")! });
