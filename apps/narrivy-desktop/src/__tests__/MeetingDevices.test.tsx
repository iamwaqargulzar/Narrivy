import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { MeetingDevices } from "../app/components/MeetingDevices";

const options = {
  auto_notes: false,
  microphone: "",
  speaker: "",
  microphones: [{ name: "headset.mic", description: "Headset microphone" }],
  speakers: [
    { name: "speakers", description: "Speakers" },
    { name: "headset.out", description: "Headset" },
  ],
  microphone_in_use: "Built-in microphone",
  speaker_in_use: "Speakers",
};
const controls = vi.hoisted(() => ({
  meetingOptions: vi.fn(),
  setMeetingDevices: vi.fn(),
  setMeetingScreen: vi.fn(),
  report: vi.fn(),
}));
vi.mock("../ui/primitives/Toast", () => ({
  useErrorHandler: () => controls.report,
}));
vi.mock("../lib/bridge", () => ({ bridge: controls }));

describe("Meeting devices", () => {
  it("shows what a meeting will record and changes the call's output", async () => {
    controls.meetingOptions.mockResolvedValue(options);
    controls.setMeetingDevices.mockResolvedValue({
      ...options,
      speaker: "headset.out",
      speaker_in_use: "Headset",
    });
    render(<MeetingDevices recording={false} />);
    expect(
      await screen.findByText(
        "Records Built-in microphone and call audio from Speakers.",
      ),
    ).toBeInTheDocument();
    fireEvent.change(screen.getByRole("combobox", { name: "Call audio" }), {
      target: { value: "headset.out" },
    });
    await waitFor(() =>
      expect(controls.setMeetingDevices).toHaveBeenCalledWith({
        speaker: "headset.out",
        call_audio: true,
      }),
    );
    expect(
      await screen.findByText(
        "Records Built-in microphone and call audio from Headset.",
      ),
    ).toBeInTheDocument();
  });

  it("can leave the call's sound out", async () => {
    controls.meetingOptions.mockResolvedValue(options);
    controls.setMeetingDevices.mockResolvedValue({
      ...options,
      call_audio: false,
    });
    render(<MeetingDevices recording={false} />);
    fireEvent.change(
      await screen.findByRole("combobox", { name: "Call audio" }),
      {
        target: { value: "!off" },
      },
    );
    await waitFor(() =>
      expect(controls.setMeetingDevices).toHaveBeenCalledWith({
        call_audio: false,
      }),
    );
    expect(
      await screen.findByText(
        "Records Built-in microphone only; call audio is off.",
      ),
    ).toBeInTheDocument();
  });

  it("can leave the microphone out to transcribe a video", async () => {
    controls.meetingOptions.mockResolvedValue(options);
    controls.setMeetingDevices.mockResolvedValue({
      ...options,
      room_audio: false,
    });
    render(<MeetingDevices recording={false} />);
    fireEvent.change(
      await screen.findByRole("combobox", { name: "Microphone" }),
      { target: { value: "!off" } },
    );
    await waitFor(() =>
      expect(controls.setMeetingDevices).toHaveBeenCalledWith({
        room_audio: false,
      }),
    );
    expect(
      await screen.findByText(
        "Records call audio from Speakers only; the microphone is off.",
      ),
    ).toBeInTheDocument();
    // The call audio cannot be turned off too.
    expect(
      screen.queryByRole("option", { name: "Off (microphone only)" }),
    ).not.toBeInTheDocument();
  });

  it("locks the devices while a meeting records", async () => {
    controls.meetingOptions.mockResolvedValue(options);
    render(<MeetingDevices recording />);
    expect(
      await screen.findByText(
        /Recording Built-in microphone and call audio from Speakers\./,
      ),
    ).toBeInTheDocument();
    for (const select of screen.getAllByRole("combobox")) {
      expect(select).toBeDisabled();
    }
  });

  it("keeps what is on screen when asked, and explains when it cannot", async () => {
    controls.meetingOptions.mockResolvedValue({ ...options, screen: false, screen_available: true });
    controls.setMeetingScreen.mockResolvedValue({ ...options, screen: true, screen_available: true });
    render(<MeetingDevices recording={false} />);
    const toggle = await screen.findByRole("switch", { name: "Keep what is on screen" });
    expect(toggle).toHaveAttribute("aria-checked", "false");
    fireEvent.click(toggle);
    await waitFor(() => expect(controls.setMeetingScreen).toHaveBeenCalledWith(true));
    await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "true"));
  });

  it("cannot keep the screen without the PipeWire plugin", async () => {
    controls.meetingOptions.mockResolvedValue({ ...options, screen: false, screen_available: false });
    render(<MeetingDevices recording={false} />);
    const toggle = await screen.findByRole("switch", { name: "Keep what is on screen" });
    expect(toggle).toBeDisabled();
    expect(toggle.parentElement).toHaveAttribute("title", expect.stringContaining("gstreamer1.0-pipewire"));
  });
});
