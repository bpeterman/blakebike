import { cleanup, render, screen } from "@testing-library/react";
import type { TooltipPayloadEntry } from "recharts";
import { afterEach, describe, expect, it } from "vitest";
import { ChartTooltip } from "./ChartTooltip";
import { chartTooltipColors } from "./chartTooltipTheme";

afterEach(cleanup);

const power: TooltipPayloadEntry = { graphicalItemId: "power", name: "powerWatts", value: 174, color: "#c8ff32" };

describe("ChartTooltip", () => {
  it("renders nothing until Recharts reports an active point", () => {
    const { container } = render(
      <ChartTooltip payload={[power]} label={684_000} formatRow={() => ({ color: "#c8ff32", name: "Power", value: "174 W" })} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the formatted title and one row per series", () => {
    render(
      <ChartTooltip
        active
        payload={[power]}
        label={684_000}
        formatTitle={(label) => `at ${label}`}
        formatRow={(entry) => ({ color: "#c8ff32", name: "Power", value: `${entry.value} W` })}
      />,
    );
    expect(screen.getByText("at 684000")).toHaveClass("chart-tooltip-title");
    expect(screen.getByText("Power")).toHaveClass("chart-tooltip-name");
    expect(screen.getByText("174 W")).toHaveClass("chart-tooltip-value");
  });

  it("keeps the series colour on the swatch rather than the text", () => {
    const { container } = render(
      <ChartTooltip
        active
        payload={[power]}
        label="Zone 3"
        formatRow={() => ({ color: "#6ca8ff", name: "Time", value: "12:40" })}
      />,
    );
    expect(container.querySelector(".chart-tooltip-swatch")).toHaveStyle({ background: "#6ca8ff" });
    expect(screen.getByText("Zone 3")).toHaveStyle({ color: chartTooltipColors.muted });
    expect(screen.getByText("Time")).toHaveStyle({ color: chartTooltipColors.muted });
    expect(screen.getByText("12:40")).toHaveStyle({ color: chartTooltipColors.text });
  });
});
