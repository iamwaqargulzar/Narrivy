# ADR-0063: Explicit systemd cgroup ownership for daemon descendants

Status: Accepted structural package prerequisite (2026-09-04); native
abrupt-death qualification pending

## Context

Narrivyd owns resident ASR workers and narrowly resolved capture/delivery
helpers with process groups, parent-death signaling, explicit graceful
shutdown, and bounded forced cleanup. A hostile or defective descendant can
create another session and escape a process-group kill, however. Recovery C4's
systemd user service did not state its cgroup lifecycle policy explicitly, and
the historical package service had drifted to a weaker composition.

The Linux 1.0 primary lifecycle is a systemd user service. Every process
created by that service remains in its service cgroup unless delegation or an
external privileged actor moves it. Session/process-group changes do not move
a process out of the cgroup. This is the correct outer containment boundary,
but source configuration is not native proof of cleanup behavior.

## Decision

The canonical package service explicitly requires:

- `ExitType=cgroup`, so service lifetime includes all descendants rather than
  only the main daemon process;
- `KillMode=control-group`, so stop/restart cleanup targets the entire service
  cgroup even when a child escaped the daemon's process group;
- `TimeoutStopSec=3s` and `TimeoutStopFailureMode=kill`, bounding graceful
  shutdown before forced cleanup; and
- `SendSIGKILL=yes`, preventing a configured no-final-kill state from leaving
  descendants behind.

The recovery candidate and both package service templates are byte-identical.
The existing strict environment, raw-identity unsets, empty capability set,
`NoNewPrivileges`, private temporary directory, protected system/home, private
XDG runtime/state/cache directories, and `AF_UNIX` restriction remain.

Tests compare every service copy with the canonical byte sequence and reject
removal of any cgroup/timeout directive. `systemd-analyze verify` must accept
the unit on the current systemd 255 host.

## Trust and residual limits

This establishes package configuration intent only. It does not prove that a
packaged daemon and real helpers remain in the expected cgroup, that forced
cleanup succeeds after daemon crash/hang/logout, or that restart and recovery
preserve user data. Native package-artifact qualification must inspect the
actual unit properties/cgroup, create a content-free escaped-session helper,
exercise graceful and abrupt daemon death, and prove no descendant remains.

The systemd-less sidecar fallback has no equivalent cgroup authority and
remains separately unqualified. Root/kernel compromise and a privileged actor
moving a process between cgroups are outside this boundary.

## Consequences

- A future replacement package has one drift-resistant user-service template
  with explicit outer descendant ownership.
- Process-group and parent-death cleanup remain necessary fast inner layers;
  cgroup cleanup is the final service-manager layer.
- B-PROCESS-014 is narrowed structurally but remains open pending native
  package-artifact observation and provenance binding.
- No service was installed or started, no process/cgroup observation was
  claimed, and no evidence, requirement, package, or release state changed.
