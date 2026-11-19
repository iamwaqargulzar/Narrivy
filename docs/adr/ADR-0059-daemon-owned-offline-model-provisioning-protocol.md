# ADR-0059: Daemon-owned offline model provisioning protocol

Status: Accepted wire contract for controlled Recovery 4Q-C5B2B1
(2026-09-03); implementation remains fail-closed and non-release

## Context

The installed daemon can truthfully remain reachable while its selected
bundled model is absent. A future local import must not let an IPC client
choose model identity, digest, destination, network source, or lifecycle
deadline. The work is long-running and cannot be represented as a synchronous
request whose connection timeout owns cancellation.

## Decision

Protocol minor 5 adds two commands: start from one bounded local source path,
and cancel one daemon-generated opaque operation identifier. The source field
is a transport shape only: 1..=4096 UTF-8 bytes with no control characters.
The operation identifier is 1..=64 ASCII letters, digits, `-`, or `_`. Both
fields are validated on serialization and deserialization and redacted from
generic debug output.

The start command deliberately carries no model ID, expected digest, cache or
destination path, URL, timeout, or policy flag. The daemon will select those
from its installed configuration and bundled registry authority.

`StatusDto` gains an optional `model_provisioning` snapshot, and start/cancel
responses use the same typed snapshot. Its state, publication certainty,
cleanup certainty, optional failure class, opaque operation ID, and monotonic
revision are explicit. Installed model lifecycle gains `provisioning`,
`published_unconfirmed`, and `activation_failed`; only `ready` represents an
owned active resident binding. All new wire enums preserve unknown future
values. Old status payloads without the optional field remain decodable.

No event is added. Clients poll status until an independently versioned event
contract exists. Clients must gate the commands on negotiated protocol minor 5.

## Security and compatibility

The wire bound does not make a source path safe. Linux descriptor-relative
resolution, ownership/type/mode policy, no-follow opening, bundled identity,
integrity verification, admission, rate limiting, lifecycle authorization, and
shutdown ownership remain daemon/adapter responsibilities.

The current daemon explicitly rejects both commands as unavailable. This keeps
the additive protocol compile-compatible without accidentally exposing a
mutation before its lifecycle owner exists.

## Consequences

- The future daemon service has a narrow, non-authority-laundering request
  shape and a truthful asynchronous status model.
- A disconnected client does not implicitly cancel daemon-owned work; explicit
  cancel and daemon shutdown will be the lifecycle controls.
- This ADR does not implement import, activation, authorization, rate limiting,
  CLI/UI, model mutation, evidence, packaging, or release behavior.

