# Security Policy

Narrivy handles unusually sensitive local capabilities: microphone audio, text insertion, clipboard/selection, application context, optional project context, and optional remote AI providers.

Security requirements are detailed in `docs/security/`.

## Default principles
- least privilege;
- local-first processing;
- no transcript/context telemetry by default;
- secret storage through platform key stores;
- signed/verified model and plugin artifacts where possible;
- explicit plugin permissions;
- network-deny by default for sandboxed plugins;
- clear user-facing cloud boundary indicators;
- privacy zones that override Mode or plugin behavior.

## Reporting

Do not publish a security vulnerability in a public issue before a private disclosure channel exists. Establish the project's official security contact before public release.
