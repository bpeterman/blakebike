import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SourceSelect } from "./SourceSelect";
import { defaultSourcePreferences, withSourcePreference } from "./sourcePreferences";

describe("SourceSelect", () => {
  afterEach(cleanup);

  it("offers every supported power source and reports changes", () => {
    const onChange = vi.fn();
    render(
      <SourceSelect
        metric="power"
        choice={{ mode: "auto" }}
        onChange={onChange}
      />,
    );

    const select = screen.getByRole("combobox", { name: "Power source" });
    expect(Array.from((select as HTMLSelectElement).options).map((option) => option.text)).toEqual([
      "Auto",
      "Power meter",
      "Trainer",
    ]);

    fireEvent.change(select, { target: { value: "trainer" } });
    expect(onChange).toHaveBeenCalledWith({ mode: "role", role: "trainer" });
  });

  it("offers the dedicated sensor, power meter, and trainer for cadence", () => {
    render(
      <SourceSelect
        metric="cadence"
        choice={{ mode: "role", role: "power" }}
        onChange={vi.fn()}
      />,
    );

    const select = screen.getByRole("combobox", { name: "Cadence source" });
    expect((select as HTMLSelectElement).value).toBe("power");
    expect(Array.from((select as HTMLSelectElement).options).map((option) => option.text)).toEqual([
      "Auto",
      "Cadence sensor",
      "Power meter",
      "Trainer",
    ]);
  });

  it("updates one preference without changing the others", () => {
    expect(
      withSourcePreference(defaultSourcePreferences, "cadence", {
        mode: "role",
        role: "trainer",
      }),
    ).toEqual({
      power: { mode: "auto" },
      cadence: { mode: "role", role: "trainer" },
      heartRate: { mode: "auto" },
    });
  });
});
