import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { HistorySettings } from "../app/components/HistorySettings";

const controls = vi.hoisted(() => ({
  setHistoryRetention: vi.fn(),
  clearHistory: vi.fn(),
  report: vi.fn(),
}));
vi.mock("../ui/primitives/Toast", () => ({
  useErrorHandler: () => controls.report,
}));
vi.mock("../lib/bridge", () => ({ bridge: controls }));

describe("History settings", () => {
  it("changes how long dictations are kept", async () => {
    const changed = vi.fn();
    render(<HistorySettings retention="7d" count={3} onChanged={changed} />);
    fireEvent.change(screen.getByRole("combobox"), {
      target: { value: "30d" },
    });
    await waitFor(() =>
      expect(controls.setHistoryRetention).toHaveBeenCalledWith("30d"),
    );
    expect(changed).toHaveBeenCalled();
  });

  it("asks before clearing, and can be kept", async () => {
    const changed = vi.fn();
    render(<HistorySettings retention="7d" count={3} onChanged={changed} />);
    fireEvent.click(screen.getByRole("button", { name: "Clear history" }));
    expect(
      screen.getByText("Delete all 3 entries? This cannot be undone."),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Keep them" }));
    expect(controls.clearHistory).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Clear history" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete all" }));
    await waitFor(() => expect(controls.clearHistory).toHaveBeenCalledTimes(1));
  });
});
