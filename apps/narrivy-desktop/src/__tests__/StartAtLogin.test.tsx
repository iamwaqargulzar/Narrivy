import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { StartupCard } from "../app/components/SettingsCompute/StartupCard";

const controls = vi.hoisted(() => ({
  autostart: vi.fn(),
  setAutostart: vi.fn(),
  report: vi.fn(),
}));
vi.mock("../ui/primitives/Toast", () => ({
  useErrorHandler: () => controls.report,
}));
vi.mock("../lib/bridge", () => ({ bridge: controls }));

describe("start at login", () => {
  it("turns on, then starts in the tray", async () => {
    controls.autostart.mockResolvedValue({
      enabled: false,
      hidden: false,
      service_installed: true,
    });
    controls.setAutostart.mockImplementation(
      async (enabled: boolean, hidden: boolean) => ({
        enabled,
        hidden,
        service_installed: true,
      }),
    );
    render(<StartupCard />);
    const login = await screen.findByRole("switch", { name: "Start at login" });
    const tray = screen.getByRole("switch", { name: "Start in the tray" });
    await waitFor(() => expect(login).toBeEnabled());
    expect(tray).toBeDisabled();
    fireEvent.click(login);
    await waitFor(() => expect(login).toHaveAttribute("aria-checked", "true"));
    expect(controls.setAutostart).toHaveBeenLastCalledWith(true, false);
    fireEvent.click(tray);
    await waitFor(() =>
      expect(controls.setAutostart).toHaveBeenLastCalledWith(true, true),
    );
  });

  it("says when no service is installed", async () => {
    controls.autostart.mockResolvedValue({
      enabled: true,
      hidden: false,
      service_installed: false,
    });
    render(<StartupCard />);
    expect(
      await screen.findByText(/service is not installed/),
    ).toBeInTheDocument();
  });
});
