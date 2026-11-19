import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { HoldKeyCard } from "../app/components/SettingsCompute/HoldKeyCard";

const controls = vi.hoisted(() => ({
  holdKey: vi.fn(),
  setHoldKey: vi.fn(),
  report: vi.fn(),
}));
vi.mock("../ui/primitives/Toast", () => ({
  useErrorHandler: () => controls.report,
}));
vi.mock("../lib/bridge", () => ({ bridge: controls }));

const keys = [{ code: 97, name: "Right Ctrl" }];

describe("press and hold", () => {
  it("is off until a key is chosen, and explains the keyboard access first", async () => {
    controls.holdKey.mockResolvedValue({ key: null, state: "off", keys });
    controls.setHoldKey.mockResolvedValue({ key: 97, state: "reading", keys });
    render(<HoldKeyCard />);
    expect(screen.getByText(/reads your keyboards/)).toBeInTheDocument();
    const select = await screen.findByRole("combobox", { name: "Key to hold" });
    await waitFor(() => expect(select).toHaveValue(""));
    fireEvent.change(select, { target: { value: "97" } });
    await waitFor(() => expect(controls.setHoldKey).toHaveBeenCalledWith(97));
    expect(
      await screen.findByText("On: hold the key to dictate."),
    ).toBeInTheDocument();
  });

  it("says why when the keyboards cannot be read", async () => {
    controls.holdKey.mockResolvedValue({ key: 97, state: "denied", keys });
    render(<HoldKeyCard />);
    expect(
      await screen.findByText(/not in the “input” group/),
    ).toBeInTheDocument();
  });
});
