import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { OverlayView } from "../lib/bridge";

let send: ((view: OverlayView) => void) | undefined;

vi.mock("../lib/bridge", () => ({
  onOverlayView: (handler: (view: OverlayView) => void) => {
    send = handler;
    return Promise.resolve(() => undefined);
  },
}));

const { Overlay, elapsedLabel, textTail } = await import("../overlay/Overlay");

const palette = {
  surfaceTop: "#111",
  surfaceBottom: "#000",
  border: "#222",
  highlight: "#333",
  shadow: "#000",
  textStrong: "#fff",
  mutedStrong: "#ccc",
  muted: "#999",
  accent: "#eee",
  accentStrong: "#fff",
  live: "#f66",
  err: "#f00",
};

function view(over: Partial<OverlayView>): OverlayView {
  return { mode: "listening", label: null, text: null, listeningMs: 0, top: false, palette, ...over };
}

describe("web-view overlay", () => {
  it("draws nothing until the shell sends a view, then the state it is sent", async () => {
    const { container } = render(<Overlay />);
    expect(container.firstChild).toBeNull();
    await act(async () => send?.(view({ listeningMs: 65_000 })));
    expect(screen.getByText("1:05")).toBeTruthy();
    expect(container.querySelector(".ov-card.is-listening")).toBeTruthy();

    await act(async () => send?.(view({ mode: "error", label: "Dictation failed", listeningMs: null })));
    expect(screen.getByRole("status").textContent).toContain("Dictation failed");
    expect(screen.queryByText("1:05")).toBeNull();
  });

  it("keeps long text to its newest words and formats the timer", () => {
    expect(elapsedLabel(0)).toBe("0:00");
    expect(elapsedLabel(600_000)).toBe("10:00");
    const long = Array.from({ length: 30 }, (_, i) => `w${i}`).join(" ");
    expect(textTail(long).startsWith("… w14 ")).toBe(true);
    expect(textTail("short text")).toBe("short text");
  });
});
