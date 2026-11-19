import { describe, expect, it } from "vitest";
import { modelFailureReason } from "../app/screens/Pipelines";
import { t } from "../i18n";

describe("a model that did not load says why in plain words", () => {
  it("names a slow computer instead of the startup code", () => {
    const reason = modelFailureReason("resident_worker_startup_timeout");
    expect(reason).not.toContain("resident_worker_startup_timeout");
    expect(reason).toContain("took too long");
    expect(t("pipe.model.failed", { reason })).toContain(
      "The previous model is still in use.",
    );
  });

  it("explains memory and device failures", () => {
    expect(modelFailureReason("resident_out_of_memory")).toContain("memory");
    expect(modelFailureReason("resident_native_backend_failed")).toContain(
      "device",
    );
  });

  it("still names an unknown code so a report can find it", () => {
    expect(modelFailureReason("something_new")).toContain("something_new");
  });
});
