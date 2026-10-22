# Module Store

> **Status: design approved by the owner on 2026-09-30 (D23). First slice
> implemented on 2026-10-01 (ADR-0078):**
> - the OpenVINO runtime pack and the OpenVINO speech-model packs, with the
>   daemon's "fits this computer" verdict;
> - install with verified multi-file packs, and removal;
> - the Store screen.
>
> **Not done yet:**
> - the signed catalog snapshot, updates and rollback;
> - the self-test step at install time;
> - noise-suppression and refinement modules;
> - "Add…" from a pipeline stage. It builds on accepted decisions and does not replace them: ADR-0014 (module boundary), ADR-0015 (lean base, runtime packs),
> ADR-0016 (pipelines), ADR-0017 (resource registry and trust), ADR-0018
> (text post-processing), and [resource-registry.md](resource-registry.md).

## Why

One installer cannot carry every module. A noise-suppression model built for
an NPU is dead weight on a computer without one, and a small language model
for refinement is hundreds of megabytes most users will never want. The
base install stays lean (ADR-0015); everything else is added by the user,
for their hardware, from a store, and kept up to date.

## What the user sees

1. **Store** (new sidebar entry). Accelerators and modules, grouped by
   what they do (owner direction D27, 2026-10-01):
   - **Accelerators:** Vulkan is shown as built in (nothing to install);
     OpenVINO installs; CUDA is listed as coming later, with whether this
     computer has an NVIDIA GPU.
   - **Refine:** extensions that polish dictated text: clean-up by a
     language model, grammar and style check, neural punctuation, offline
     translation. The language models themselves are not in the Store
     (D32): they are on the Models screen under **Language models**, and a
     Refine extension links there for its model.
   - **Noise suppression**, and later other modules and pipeline recipes.
   - **Extensions** (D33): what brings Narrivy into other apps or adds a
     whole area: the browser writing assistant, the AI assistant, meeting
     transcription and the voice changer. Each is turned on here; its
     models come from the Models screen.
   - **Models are never in the Store.** Speech models and language models
     are found, installed and removed on the Models screen, in two
     separate sections (**Speech to text** and **Language models**, D32);
     a model that needs an add-on links to the Store for it.

   Each item shows name, what it does, size, licence, who verified it, and
   **whether it fits this computer**. Planned items are shown as "Coming
   later" and cannot be installed.
2. **Add from a pipeline stage.** Each stage on the Pipelines screen has
   "Add…". It opens the Store filtered to that stage, and after install the
   module appears in that stage's choices.
3. **Transcribe** keeps today's logic: it lists the speech models installed
   on the Models screen. The Store is where more are found.
4. **Refine** (new sidebar entry). Two parts, kept separate (ADR-0018):
   - *Rules* (deterministic, always local): punctuation and capitals,
     numbers, vocabulary, replacements.
   - *Language model* (optional): pick a model installed under Models →
     Language models, where
     it runs, and how far it may change the text (faithful cleanup, polish,
     rewrite). The meaning check guards its output.

## The four kinds of item (ADR-0017)

| Kind | Example | Runs code? |
|---|---|---|
| Model pack | a speech model, a denoise model, a small language model | No, data only |
| Runtime pack | an accelerator runtime for an NPU or GPU | Yes, trusted |
| Module pack | a noise-suppression implementation | Yes, isolated worker |
| Pipeline recipe | "Noisy office" as a ready-made pipeline | No, configuration |

A module is often three items: the module, the model it loads, and the
runtime it needs. The Store installs the set as one action and shows the
total size first.

## "Fits this computer"

The daemon already discovers devices and plans compute. Each store item
declares what it needs: device class (NPU, discrete GPU, integrated GPU,
CPU), runtime pack, memory, architecture. The daemon, not the UI, compares
the two and reports one of:

- **Fits**: requirements met now.
- **Fits after adding a runtime**: shows the runtime and its size.
- **Does not fit**: with the reason ("needs an NPU; none found").
- **Unknown until tested**: hardware is present but this exact combination
  has not been proven here (enumeration is not a support claim, ADR-0015).

The default view shows only items that fit. "Show everything" reveals the
rest, each with its reason. An item that does not fit cannot be installed
outside developer mode.

## Install

Resolve → download to staging → verify size and SHA-256 (and signature where
present) → check licence and archive paths → self-test → qualify on this
computer → publish atomically → make available to pipelines. A failure at
any step leaves what was there before untouched (ADR-0017).

States shown to the user: Available, Downloading (pause, resume, cancel),
Verifying, Testing on this computer, Installed, In use by N pipelines,
Update available, Failed (with the reason and a retry), Removing.

Every download is started by the user. Narrivy never goes online on its own
(update policy, T2.17).

## Updates

- The store catalog is a versioned, signed snapshot. "Check for updates" is
  a user action; it fetches the snapshot and compares.
- A new version installs **beside** the current one, is self-tested and
  qualified, and only then becomes active, between dictations, never during
  one (ADR-0016).
- The previous version is kept until the new one has completed a dictation
  successfully; a failure rolls back automatically and says so.
- A pipeline pins the major version of each item it uses. A major update is
  offered, not applied, and shows what changes.
- The snapshot carries a revocation list. A revoked item (security or
  licence) is disabled at the next check, the pipeline falls back, and the
  user is told why.
- Items are shared and reference-counted: five pipelines using one model
  hold one copy. Removing the last user offers to free the space.

## Trust

Built-in, Narrivy Verified, Upstream Signed, Community, Local/Developer
(ADR-0017). 1.0 shows only Built-in and Narrivy Verified. Module packs never
load into the daemon as libraries (ADR-0014): native code runs as an
isolated worker with a digest-bound binary, as the speech worker does today;
text rules can run as sandboxed WASM. Network access for a module is denied
by default.

## Edge cases

| Case | Behaviour |
|---|---|
| Download interrupted, network lost, app closed | Resumable; staging is kept and re-verified; nothing half-installed is ever active |
| Disk full | Checked before download with the full set's size; fails early with the space needed |
| Hash or signature mismatch | Discarded, reported, never retried silently from another source |
| Self-test or qualification fails | Not activated; shown as "Does not work on this computer" with the step that failed; user can remove it |
| Hardware removed or driver changed after install | Re-qualification on next start; the pipeline falls back down NPU → GPU → CPU and says which it used |
| Module crashes during dictation | Worker is isolated: the stage is skipped per the pipeline's fallback policy, dictation still delivers, the module is marked degraded |
| Update breaks a working setup | Automatic rollback to the kept previous version |
| Item in use is removed | Blocked with the list of pipelines using it, or remove-and-fall-back after confirmation |
| Two modules need different versions of one runtime | Runtimes are versioned side by side; each module binds to its own |
| Runtime update | Invalidates compiled caches (NPU blobs); re-qualify every dependent item before switching |
| Offline computer | Import a signed offline bundle from a file; same verification |
| Gated or non-redistributable model | Fetched from its owner after the user authorises; never mirrored |
| Cloud refinement | A privacy boundary change: opt-in per pipeline, clearly marked, never a fallback from local |
| Two heavy models on one accelerator | The scheduler qualifies the whole plan (ADR-0018); if memory is short it proposes moving one to another device |
| Catalog entry disappears upstream | Installed copy keeps working; marked "no longer available", no updates |
| Same item installed twice (store and local import) | Deduplicated by content hash |
| Pipeline recipe needs items not installed | Import shows the missing set and total size; nothing activates until it is complete |
| Base install with no network ever | Works: CPU model and rules are built in |

## Phasing

1. **1.0:** speech models and accelerator runtimes from the Narrivy Hugging
   Face organisation (T2.17, T2.15, T2.16) behind the Store and "fits this
   computer"; the built-in light noise cleanup; the Refine screen with its
   rules. Narrivy Verified only.
2. **Next:** noise-suppression modules, then local refinement language
   models.
3. **Later:** community items and third-party publishers (post-1.0 by
   ADR-0017).

## Accelerator runtimes (D25, 2026-09-30)

Vulkan acceleration is part of the base install (ADR-0076): it is not a
Store item. The Store offers the other accelerator runtimes as add-ons:

- **OpenVINO** (Intel NPU and GPU), needed by the OpenVINO model packs;
- **CUDA** (NVIDIA).

Each is an optional runtime pack. A model that needs one shows "needs an
add-on" until that pack is installed and has passed its self-test.

## Owner decisions (D23, 2026-09-30)

1. Store and Refine are sidebar entries.
2. Noise suppression: a built-in light cleanup in 1.0; hardware-specific
   modules through the Store after 1.0.
3. Refinement language model: local only first.
4. 1.0 shows Narrivy Verified items only.
5. Everything is hosted in the Narrivy Hugging Face organisation.

## Where it is hosted

One repository, `narrivy/store`, laid out by kind and runtime (owner
request, 2026-10-01: a hierarchy, not one repository per item):

```text
models/gguf/<pack>/          transcribe.cpp models (CPU or Vulkan GPU)
models/openvino/<pack>/      OpenVINO models (NPU, GPU or CPU)
runtimes/openvino/<pack>/    the OpenVINO add-on
modules/<runtime>/<module>/  add-on modules, as they are published
```

- The catalog names each item's repository, folder (`source_path`) and
  pinned revision; the manifest in the folder pins every file.
- `tools/reference-models/build_model_packs.py` stages the tree in
  `~/.cache/narrivy/hf-staging/store` with an index README and `upload.sh`.
  The same tree is the developer source (`NARRIVY_STORE_LOCAL_DIR`).
- GGUF folders are named by format, not by GPU API: the same file runs on
  the CPU and Vulkan today, and CUDA and Metal later.
