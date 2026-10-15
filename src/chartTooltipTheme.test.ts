import { describe, expect, it } from "vitest";
import { chartTooltipColors, contrastRatio } from "./chartTooltipTheme";

describe("contrastRatio", () => {
  it("matches the WCAG reference values", () => {
    expect(contrastRatio("#ffffff", "#000000")).toBeCloseTo(21, 5);
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 5);
    expect(contrastRatio("#777777", "#ffffff")).toBeCloseTo(4.48, 2);
    expect(contrastRatio("#3a4137", "#3a4137")).toBe(1);
  });

  it("rejects anything but #rrggbb", () => {
    expect(() => contrastRatio("#fff", "#000000")).toThrow(/#rrggbb/);
  });
});

describe("chart tooltip colours", () => {
  it.each(["text", "muted"] as const)("%s meets WCAG AA against the tooltip background", (key) => {
    expect(contrastRatio(chartTooltipColors[key], chartTooltipColors.background)).toBeGreaterThanOrEqual(4.5);
  });
});
