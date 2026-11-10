import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ProcessOnStopSwitch } from "../app/components/AutoNotesSwitch";

const controls = vi.hoisted(() => ({
  meetingOptions: vi.fn(),
  setProcessOnStop: vi.fn(),
  report: vi.fn(),
}));
vi.mock("../ui/primitives/Toast", () => ({
  useErrorHandler: () => controls.report,
}));
vi.mock("../lib/bridge", () => ({ bridge: controls }));

describe("Going over recordings when they stop", () => {
  it("is off by default and switches on", async () => {
    controls.meetingOptions.mockResolvedValue({ auto_notes: false, process_on_stop: false });
    controls.setProcessOnStop.mockResolvedValue({ auto_notes: false, process_on_stop: true });
    render(<ProcessOnStopSwitch />);
    const toggle = await screen.findByRole("switch", { name: "Go over recordings when they stop" });
    expect(toggle).toHaveAttribute("aria-checked", "false");
    fireEvent.click(toggle);
    await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "true"));
    expect(controls.setProcessOnStop).toHaveBeenCalledWith(true);
  });

  it("is not shown by a service that always goes over them", async () => {
    controls.meetingOptions.mockResolvedValue({ auto_notes: false });
    const { container } = render(<ProcessOnStopSwitch />);
    await waitFor(() => expect(controls.meetingOptions).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });
});
