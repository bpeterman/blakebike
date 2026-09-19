import "@testing-library/jest-dom/vitest";

// jsdom has no layout, so size-tracking hooks keep their fallback size. A
// no-op ResizeObserver lets the observe/disconnect path run in tests.
if (typeof globalThis.ResizeObserver === "undefined") {
  class NoopResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver = NoopResizeObserver as unknown as typeof ResizeObserver;
}
