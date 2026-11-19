# Linux Release and Packaging

## 1.0 artifact priority

1. `.deb` — first-class Ubuntu/Pop!_OS artifact.
2. AppImage — follow-on after 1.0 only (owner decision 2026-08-22: single-artifact `.deb` release; re-promotion requires an ADR).
3. RPM/AUR/Nix — follow-on unless promoted without delaying 1.0.
4. Flatpak — only after proving system-wide dictation remains a good sandbox/portal experience.

## Package contract

Base package includes desktop integration, icons/metadata, executable(s), daemon lifecycle integration, migrations, runtime dependency declarations, reference CPU correctness support, runtime-pack manager/discovery, and version/build identity. It must not bundle unrelated vendor accelerator stacks merely for possible future hardware. Per-user data/config/cache follows documented XDG locations.

Installation must not require broad permanent root privileges beyond the package-manager operation. Runtime should run as the user.

## Data lifecycle

- upgrade preserves/migrates user configuration/history/model metadata transactionally;
- failed/interrupted migration retains recoverable previous state;
- uninstall removes program files/service registrations;
- both `apt remove` and `apt purge` remove package-owned files while preserving
  every user's XDG config, data, state, history, and model cache; maintainer
  scripts never enumerate or traverse home directories;
- deleting user-owned Narrivy data is a separate, explicit action performed in
  that user's session. Release notes document the retained XDG paths and manual
  removal command until a backup-first in-app purge flow is qualified.

## AppImage compatibility

Build on a sufficiently compatible Linux baseline and test the resulting binary on clean target systems; Linux portability is constrained by the build environment and bundled/unbundled native dependencies. Do not infer portability from a successful build on a newer development distro.

## Release materials

Checksums, SBOM, dependency/license inventory, third-party notices, build identity, support matrix, known limitations, and signing/provenance where available.


## Lean provisioning

Large model weights and vendor accelerator runtimes may be separate verified downloads. First-run provisioning must clearly distinguish application install size from model/runtime-pack download size, honor network/privacy policy, support resumable staged acquisition, and preserve a CPU path. Publish an optional offline bundle if fully offline initial provisioning is desired; do not bloat the normal installer solely for that case.

Runtime packs are independently removable when no installed/selected model depends on them. Uninstall/purge behavior must distinguish application files, user data, source models, optimized caches and optional runtime packs.
