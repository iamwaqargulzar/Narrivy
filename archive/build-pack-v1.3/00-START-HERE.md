# Narrivy — Project Start

For autonomous implementation, begin with **`00-AUTONOMOUS-START.md`**.

## Product principles

1. Greenfield implementation; references are behavioral only.
2. Portable Rust domain core; platform/model frameworks live behind contracts.
3. Linux 1.0 is first-class on Pop!_OS/COSMIC and Ubuntu/GNOME Wayland.
4. Capability-driven behavior instead of scattered OS checks.
5. Local-first privacy and explicit processing boundaries.
6. Reliable dictation before feature breadth.
7. Premium UI/UX, accessibility, diagnostics, and recovery are part of feature completion.
8. Measure the entire trigger-to-committed-text path.
9. Safe, reversible editing and bounded fallbacks.
10. Every phase has an objective gate and evidence.

## Read order for humans

1. `docs/01-product/product-requirements.md`
2. `docs/02-architecture/system-overview.md`
3. `docs/09-phases/`
4. `docs/03-ui-ux/`
5. `docs/04-platforms/`
6. `docs/05-ml-inference/`
7. `docs/06-security-privacy/`
8. `docs/07-testing/`
9. `docs/16-release/`


## v1.3 canonical architecture

Read `docs/00-ARCHITECTURE-MAP.md` and `spec/catalogs/` before changing pipelines, engines, modules, runtimes, models or providers.
