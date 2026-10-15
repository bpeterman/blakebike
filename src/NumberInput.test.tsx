import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { EditableNumberInput } from "./EditableNumberInput";

afterEach(cleanup);

describe("EditableNumberInput", () => {
  it("allows the current number to be erased while entering a replacement", () => {
    const onValueChange = vi.fn();
    render(<EditableNumberInput aria-label="FTP" value={200} onValueChange={onValueChange} />);
    const input = screen.getByRole("spinbutton", { name: "FTP" });

    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "" } });
    expect(input).toHaveValue(null);
    expect(onValueChange).not.toHaveBeenCalled();

    fireEvent.change(input, { target: { value: "245" } });
    expect(input).toHaveValue(245);
    expect(onValueChange).toHaveBeenLastCalledWith(245);
  });

  it("restores the last value when left empty", () => {
    render(<EditableNumberInput aria-label="FTP" value={200} onValueChange={vi.fn()} />);
    const input = screen.getByRole("spinbutton", { name: "FTP" });
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "" } });
    fireEvent.blur(input);
    expect(input).toHaveValue(200);
  });
});
