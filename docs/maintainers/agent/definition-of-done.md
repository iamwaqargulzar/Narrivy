# Definition of Done

A feature/phase is done only when all applicable items below are true **and the executable phase gate passes**.

## Requirement/evidence
- requirement IDs identified in traceability;
- acceptance behavior implemented, including important failure/cancel paths;
- required automated/manual verification executed;
- evidence exists in the repository artifact/evidence structure;
- requirement status is not changed to `passed` before evidence exists.

## Architecture
- code lives in the correct layer/dependency direction;
- replaceable capability families use module contracts/registry rather than concrete core coupling;
- no platform/runtime/framework leakage into portable core;
- asynchronous outputs have transaction/ordering/cancellation semantics where needed;
- persistent/schema changes are versioned and migrated;
- material architecture change has an ADR.

## Privacy/security
- data fields and processing boundary classified;
- remote/network transfer passes policy before use;
- logs/support bundles reviewed for content/secrets;
- IPC/import/plugin/model permission/integrity boundaries enforced as applicable;
- failure/fallback does not broaden permissions silently.

## UX/accessibility
- loading/empty/ready/degraded/permission/error/recovery states;
- keyboard/focus/semantics;
- scaling/high contrast/reduced motion where applicable;
- nontechnical recovery message + diagnostics detail;
- undo/recovery for destructive transformations;
- no nonfunctional “coming soon” control presented as working.

## Reliability
- owner/lifetime/cancellation/shutdown defined for background work;
- process/device/backend failure behavior tested appropriately;
- no silent recoverable-text loss or duplicate delivery;
- audio/shortcut/clipboard temporary state restoration reviewed.

## Performance
- no blocking disk/network/inference on audio/UI critical threads;
- affected trigger→commit stage measured when performance-sensitive;
- model/cache/memory impact considered and benchmark evidence attached where required;
- benchmark-generated optimized artifacts have ownership/reference metadata and losing disposable variants are cleaned safely without removing source/selected/CPU-fallback assets.

## Regression
- unit/contract/integration tests pass;
- formatter/lints/boundary/security checks pass;
- relevant real platform/device tests pass;
- from P2 onward, every completed working-app phase has explicit real `smoke_commands`; from P6 onward the daily-use dictation smoke path still passes.

Documentation and durable agent state are updated in the same work.
