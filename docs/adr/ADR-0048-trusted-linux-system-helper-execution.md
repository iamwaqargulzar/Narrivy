# ADR-0048: Trusted Linux system-helper execution

- Status: Accepted for controlled Recovery 4P implementation
- Date: 2026-08-31
- Release effect: none; all Linux 1.0 requirements remain blocked

## Context

The active strict recovery paths execute two operating-system helpers:
`pw-record` carries microphone PCM on stdout and `wtype` receives corrected
text on stdin. Both were discovered through the daemon's inherited `PATH`,
checked only as executable regular files, then opened later by pathname.
They inherited the daemon environment, and cancellation killed only the
direct child PID.

The repository contains other subprocess families, including the resident ASR
worker, legacy sidecar, generic delivery helpers, PipeWire monitor, trigger and
audio-policy probes, and compute qualification. Migrating every family in one
change would mix distinct trust, payload, and lifecycle contracts. Recovery 4P
therefore addresses only strict `pw-record` and strict `wtype`.

On the current development host, `/usr/bin/pw-record` is a root-owned symlink to
`/usr/bin/pw-cat`. The alias is intentional and may select behavior through
`argv[0]`; canonical validation must not replace the launch alias.

## Decision

1. Add a concrete Linux process-infrastructure adapter. It owns trusted system
   helper resolution, environment construction, child process-group setup,
   parent-death setup, and bounded group termination. It contains no capture,
   delivery, ASR, policy, or daemon decisions.
2. Strict system helpers are resolved only from a reviewed absolute system
   directory set (`/usr/bin`, then `/bin`); the daemon's inherited `PATH` is
   not a discovery input. The absolute launch alias and canonical target are
   validated separately. The alias and canonical-target identities are
   snapshotted. Their relevant ancestry is not identity-pinned; its ownership
   and mode are rechecked. Both leaves and ancestry must be root-owned and not
   group/world writable; the executable target must also be executable and
   free of set-ID bits. Safe root-owned aliases are retained for execution.
3. Resolution records both alias and canonical-target identity. Every command
   construction revalidates them before spawn. Trust or identity drift is a
   typed failure and cannot reach capture or transcript dispatch.
4. Helper commands clear the inherited environment. PipeWire capture admits
   only `XDG_RUNTIME_DIR`, `PIPEWIRE_RUNTIME_DIR`, and `PIPEWIRE_REMOTE`, which
   select the same-user session endpoint, and forces `PIPEWIRE_NO_CONFIG=true`;
   code, plugin, and config-loading override variables remain absent. Wtype
   admits only `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY`. Dynamic-loader
   variables, arbitrary `PATH`, secrets, locale, and the daemon working
   directory are not inherited. A one-shot wrapper permits only argv and stdio
   configuration, defaults every unspecified stream to null, and prevents call
   sites from weakening the environment or lifecycle setup.
5. Every migrated helper starts in its own process group. Parent-death setup
   and an expected-parent race check fail spawn closed. Forced cleanup signals
   the group and boundedly reaps the direct child under the caller's existing
   absolute deadline. A successful leader which leaves a group descendant is a
   typed failure after cleanup. Cleanup uncertainty is never reported as clean
   cancellation or successful delivery. Kernels without the required
   close-range CLOEXEC primitive fail readiness closed.
6. Readiness and daemon capability reporting use the same trusted resolver.
   They report helper readiness, not microphone signal, target retention, or
   behavioral qualification. The user-facing check named `mic` therefore
   remains unavailable even when the helper is trusted; its detail reports
   helper readiness and that signal/routing are unverified.
7. Existing wtype semantics remain: payload bytes travel only through stdin;
   cancellation before a positive write is clean only after bounded cleanup;
   cancellation after a positive write remains dispatch-uncertain and requires
   exactly one Inbox recovery.

## Trust boundary

`SystemOwnedRevalidated` means a helper is rooted in a non-writable root-owned
system path and its recorded pathname identity is unchanged at the enforced
spawn boundary. This is pathname revalidation, not FD-bound execution; it does
not resist a root-authorized in-place race or independently prove distribution
provenance, package signatures, SBOM membership, or release-artifact identity.
Recovery 4P does not satisfy SEC-007 and creates no release evidence.

Process groups contain ordinary descendants that retain the helper group. They
are not a hostile-code sandbox: a malicious helper can attempt a new session
or use other process-escape mechanisms. Package provenance plus systemd
control-group qualification remain required for release.

## Unsafe invariants

The Linux-only adapter permits `unsafe` only around credential reads and the
child `pre_exec` syscall sequence. Required close-range support is checked in
the parent first. Between fork and exec, the closure performs only raw
`prctl`, `getppid`, `umask`, and `close_range` operations and constructs only a
raw-errno error; it allocates no Rust value and takes no lock. Failure aborts
spawn. These invariants are local process-hardening claims, not a sandbox or
package-provenance claim.

## Alternatives rejected

- Continue using `Command::new("pw-record")`/`Command::new("wtype")`: inherited
  `PATH` and environment remain mutable control inputs.
- Reject every symlink: this disables the legitimate PipeWire `pw-record`
  alias on the current reference host.
- Execute the canonical `pw-cat` target: this can change alias-selected
  behavior.
- Generalize process ownership into a portable domain API: Linux execution
  mechanics are infrastructure, not a portable product contract.
- Claim process groups are complete sandboxing or package integrity: neither
  claim follows from the implemented controls.

## Deferred work

- resident-worker executable and model digest/descriptor binding plus explicit
  daemon shutdown;
- legacy sidecar and generic delivery helper migration or removal;
- `pw-dump`, `busctl`, `wpctl`, and compute probe/qualifier hardening;
- systemd/cgroup abrupt-death qualification and escaped-session containment;
- package-manager provenance, SBOM binding, and clean-machine validation; and
- native microphone and target-delivery qualification.

## Primary references

- Rust `CommandExt::process_group`: <https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html>
- Linux `killpg(2)`: <https://man7.org/linux/man-pages/man2/killpg.2.html>
- Linux `PR_SET_PDEATHSIG`: <https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html>
- Linux `close_range(2)`: <https://man7.org/linux/man-pages/man2/close_range.2.html>
- Linux pathname lookup: <https://docs.kernel.org/filesystems/path-lookup.html>

## Non-goals

No native microphone run, evidence/index or requirement transition, manifest
reseal, packaging change, commit, push, or release claim.
