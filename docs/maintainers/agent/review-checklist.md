# Review Checklist

- Which requirements does this satisfy?
- Does this create a new dependency direction?
- Could this make Linux-specific behavior leak into core?
- What sensitive data enters this code path?
- What happens if the adapter/model/plugin crashes or times out?
- Can the user cancel it?
- Can the user undo/recover the result?
- What happens on suspend/resume/hotplug?
- What does a non-technical user see when it fails?
- What latency/memory cost does it add?
- Are diagnostics content-safe?
- Are docs/schema/migrations updated?
