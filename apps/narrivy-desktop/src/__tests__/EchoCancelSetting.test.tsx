import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { EchoCancelCard } from "../app/components/SettingsCompute/EchoCancelCard";

const controls = vi.hoisted(() => ({
  audioOptions: vi.fn(),
  setEchoCancel: vi.fn(),
  report: vi.fn(),
}));

vi.mock("../ui/primitives/Toast", () => ({
  useErrorHandler: () => controls.report,
}));
vi.mock("../lib/bridge", () => ({
  bridge: {
    audioOptions: controls.audioOptions,
    setEchoCancel: controls.setEchoCancel,
  },
}));

describe("ignore this computer's sound", () => {
  it("shows the daemon state and turns it off", async () => {
    controls.audioOptions.mockResolvedValue({
      echo_cancel: true,
      echo_cancel_active: true,
    });
    controls.setEchoCancel.mockResolvedValue({
      echo_cancel: false,
      echo_cancel_active: false,
    });
    render(<EchoCancelCard />);
    const toggle = await screen.findByRole("switch", {
      name: "Ignore this computer's sound",
    });
    await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "true"));
    expect(screen.getByText("On and working.")).toBeInTheDocument();
    fireEvent.click(toggle);
    await waitFor(() =>
      expect(controls.setEchoCancel).toHaveBeenCalledWith(false),
    );
    await waitFor(() =>
      expect(toggle).toHaveAttribute("aria-checked", "false"),
    );
  });

  it("is disabled when the service cannot say", async () => {
    controls.audioOptions.mockRejectedValue({ code: "incompatible_protocol" });
    render(<EchoCancelCard />);
    expect(await screen.findByText(/Not available/)).toBeInTheDocument();
    expect(screen.getByRole("switch")).toBeDisabled();
  });
});
