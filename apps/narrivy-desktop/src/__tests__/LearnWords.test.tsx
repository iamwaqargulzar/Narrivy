import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { LearnWords } from "../app/components/LearnWords";

const controls = vi.hoisted(() => ({
  vocabulary: vi.fn(),
  setVocabulary: vi.fn(),
  report: vi.fn(),
}));
vi.mock("../ui/primitives/Toast", () => ({
  useErrorHandler: () => controls.report,
}));
vi.mock("../lib/bridge", () => ({ bridge: controls }));

describe("add corrected words", () => {
  it("adds a word to the vocabulary with one click", async () => {
    controls.vocabulary.mockResolvedValue({
      words: ["Narrivy"],
      replacements: [],
    });
    controls.setVocabulary.mockImplementation(async (v) => v);
    const done = vi.fn();
    render(<LearnWords words={["Waqar"]} onDone={done} />);
    fireEvent.click(screen.getByRole("button", { name: "+ Waqar" }));
    await waitFor(() =>
      expect(controls.setVocabulary).toHaveBeenCalledWith({
        words: ["Narrivy", "Waqar"],
        replacements: [],
      }),
    );
    expect(
      await screen.findByText("Added to your words: Waqar."),
    ).toBeInTheDocument();
  });

  it("can be dismissed", () => {
    const done = vi.fn();
    render(<LearnWords words={["Waqar"]} onDone={done} />);
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(done).toHaveBeenCalled();
  });
});
