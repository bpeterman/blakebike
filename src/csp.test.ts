import { describe, expect, it } from "vitest";
import tauriConfigSource from "../src-tauri/tauri.conf.json?raw";

/** The sources one CSP directive allows, e.g. `img-src` → ["'self'", "data:", ...]. */
const cspSources = (csp: string, directive: string): string[] =>
  csp
    .split(";")
    .map((part) => part.trim().split(/\s+/))
    .find(([name]) => name === directive)
    ?.slice(1) ?? [];

describe("content security policy", () => {
  // App.css draws every <select> chevron as a data: SVG. Browsers and test
  // harnesses apply no CSP, only the Tauri webview does, so a policy that
  // blocks data: images hides the chevrons in the app and nowhere else.
  it("lets the webview load the data: images the stylesheet uses", () => {
    const { csp } = JSON.parse(tauriConfigSource).app.security;
    expect(cspSources(csp, "img-src")).toContain("data:");
  });
});
