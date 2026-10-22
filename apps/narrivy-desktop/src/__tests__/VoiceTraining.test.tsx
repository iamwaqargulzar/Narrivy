import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { VoiceCard } from "../app/components/SettingsCompute/VoiceCard";

const controls = vi.hoisted(() => ({
  audioOptions: vi.fn(),
  trainVoice: vi.fn(),
  setOnlyMyVoice: vi.fn(),
  forgetVoice: vi.fn(),
  report: vi.fn(),
}));

vi.mock("../ui/primitives/Toast", () => ({
  useErrorHandler: () => controls.report,
}));
vi.mock("../lib/bridge", () => ({ bridge: controls }));

const base = {
  echo_cancel: true,
  echo_cancel_active: true,
  voice_filter_available: true,
  only_my_voice: true,
};

describe("only my voice", () => {
  it("trains, shows the passage while recording, then offers the switch", async () => {
    controls.audioOptions.mockResolvedValueOnce({
      ...base,
      voice_trained: false,
      voice_training: "idle",
    });
    controls.trainVoice.mockResolvedValue({
      ...base,
      voice_trained: false,
      voice_training: "recording",
    });
    controls.audioOptions.mockResolvedValue({
      ...base,
      voice_trained: true,
      voice_training: "idle",
    });
    render(<VoiceCard />);
    const toggle = await screen.findByRole("switch", { name: "Only my voice" });
    expect(toggle).toBeDisabled();
    fireEvent.click(
      await screen.findByRole("button", { name: "Train my voice" }),
    );
    expect(await screen.findByText(/Read this aloud/)).toBeInTheDocument();
    await waitFor(
      () =>
        expect(
          screen.getByText("On: only your voice is written down."),
        ).toBeInTheDocument(),
      {
        timeout: 3000,
      },
    );
    expect(toggle).toBeEnabled();
    expect(toggle).toHaveAttribute("aria-checked", "true");
  });

  it("explains a training with too little speech", async () => {
    controls.audioOptions.mockResolvedValue({
      ...base,
      voice_trained: false,
      voice_training: "failed:voice_training_too_short",
    });
    render(<VoiceCard />);
    expect(
      await screen.findByText(/Not enough speech was heard/),
    ).toBeInTheDocument();
  });
});
