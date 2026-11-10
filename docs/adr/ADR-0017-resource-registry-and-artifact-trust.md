# ADR-0017 — Resource registry, artifact classes, and download trust

**Status:** Accepted  
**Date:** 2026-08-22

## Decision

Narrivy owns a small **trusted registry/catalog control plane** for resource identity, compatibility, integrity, licensing, and source priority. The registry does not need to host every byte. Where appropriate, original upstream sources are preferred and a Narrivy-controlled mirror is a verified fallback.

Resource classes are separate and independently versioned:

1. **Module Pack** — executable/sandboxed implementation of a capability.
2. **Runtime Pack** — execution technology such as a vendor accelerator runtime.
3. **Model Pack** — model weights/data and conversion metadata.
4. **Pipeline Pack/Recipe** — a portable graph/configuration that refers to required capabilities/resources.

## Source policy

- Models: prefer authoritative upstream model repositories; use the Narrivy Hugging Face organization for redistributable mirrors and Narrivy-produced conversions/optimized variants.
- Modules/runtime packs: prefer authoritative upstream releases where suitable; a Narrivy artifact/OCI-style registry may mirror or package verified releases.
- Gated or non-redistributable resources are downloaded directly from the authoritative provider after user authorization; Narrivy does not bypass licensing gates by mirroring them.
- A mutable URL is never sufficient identity. Registry entries pin version/revision plus expected hashes and, where supported, signatures/provenance.

## Install transaction

Resolve → download to staging → verify hash/signature/license/policy → validate archive paths → self-test where executable → atomically publish → update reference counts. Failure leaves the previous active resource untouched.

## Trust levels

The UI may distinguish Built-in, Narrivy Verified, Upstream Signed, Community, and Local/Developer resources. Public community repositories/marketplace remain post-1.0 unless promoted.

## Security consequence

Narrivy never blindly downloads an arbitrary executable URL and runs it. Imported archives, custom repositories, model formats that can execute code, and remote-code hooks are treated as untrusted and require stronger policy or developer mode.
