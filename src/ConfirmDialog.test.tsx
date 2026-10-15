import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ConfirmDialog } from "./ConfirmDialog";

afterEach(cleanup);

describe("ConfirmDialog", () => {
  it("focuses the dialog, closes with Escape, and restores focus", async () => {
    const before = document.createElement("button");
    document.body.append(before);
    before.focus();
    const onClose = vi.fn();
    const view = render(
      <ConfirmDialog
        title="End this ride?"
        description="The ride will be saved."
        confirmLabel="End ride"
        onConfirm={vi.fn()}
        onClose={onClose}
      />,
    );

    await waitFor(() => expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus());
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledOnce();
    view.unmount();
    expect(before).toHaveFocus();
    before.remove();
  });

  it("cannot be dismissed while its action is finishing", () => {
    const onClose = vi.fn();
    render(
      <ConfirmDialog
        title="End this ride?"
        description="The ride will be saved."
        confirmLabel="End ride"
        busy
        onConfirm={vi.fn()}
        onClose={onClose}
      />,
    );
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
  });
});
