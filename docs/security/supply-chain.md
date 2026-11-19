# Supply-chain Security

## Source dependencies

- application/workspace lockfiles are committed and CI uses `--locked`;
- remote GitHub Actions in release-critical workflows are pinned to immutable 40-hex commits; comments may name the friendly release tag;
- `cargo-deny` enforces advisories/license/source policy; policy exceptions require reason/expiry/review, not blanket allow;
- dependency updates rerun tests and relevant model/platform qualification rather than relying on semantic version promises alone;
- avoid unnecessary native/runtime dependencies that expand attack/packaging surface.

## Models/plugins/packs

Treat these as software supply-chain artifacts:
- immutable source revision/provenance;
- expected file list, sizes and SHA-256;
- signature where the distribution channel supports it;
- safe extraction rules (`SEC-006`);
- license/redistribution metadata;
- atomic install after verification;
- compiled-cache keys tied to verified source/runtime/device metadata.

## Release artifacts

P10/P11 generate:
- checksums;
- dependency/license inventory and third-party notices;
- SBOM in a standard machine-readable format chosen by release engineering;
- immutable build identity including source commit/toolchain/runtime/model catalog revisions;
- signing/provenance where secure credentials/infrastructure are available.

Signing keys never live in repository files or ordinary CI logs. Public release is blocked until the project license/legal distribution status is resolved.
