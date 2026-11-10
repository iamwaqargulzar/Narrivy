import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { VocabularyEditor } from "../app/components/VocabularyEditor";

const controls = vi.hoisted(() => ({
  vocabulary: vi.fn(),
  setVocabulary: vi.fn(),
  report: vi.fn(),
}));

vi.mock("../ui/primitives/Toast", () => ({
  useErrorHandler: () => controls.report,
}));
vi.mock("../lib/bridge", () => ({ bridge: controls }));

describe("your words", () => {
  it("adds several words at once and a replacement, and removes a word", async () => {
    controls.vocabulary.mockResolvedValue({
      words: ["Narrivy"],
      replacements: [],
    });
    controls.setVocabulary.mockImplementation(async (v) => v);
    render(<VocabularyEditor />);
    expect(await screen.findByText("Narrivy")).toBeInTheDocument();

    fireEvent.change(screen.getByRole("textbox", { name: "Add words" }), {
      target: { value: "Lahore, Waqar " },
    });
    fireEvent.click(screen.getAllByRole("button", { name: "Add" })[0]);
    await waitFor(() =>
      expect(controls.setVocabulary).toHaveBeenLastCalledWith({
        words: ["Narrivy", "Lahore", "Waqar"],
        replacements: [],
      }),
    );

    fireEvent.change(screen.getByRole("textbox", { name: "When I say" }), {
      target: { value: "btw" },
    });
    fireEvent.change(screen.getByRole("textbox", { name: "Write" }), {
      target: { value: "by the way" },
    });
    fireEvent.click(screen.getAllByRole("button", { name: "Add" })[1]);
    expect(await screen.findByText("by the way")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Remove Lahore" }));
    await waitFor(() =>
      expect(screen.queryByText("Lahore")).not.toBeInTheDocument(),
    );
  });

  it("says so when the service is too old", async () => {
    controls.vocabulary.mockRejectedValue({ code: "incompatible_protocol" });
    render(<VocabularyEditor />);
    expect(await screen.findByText(/needs updating/)).toBeInTheDocument();
  });
});
