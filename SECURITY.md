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

Please do not report security problems in a public issue. Use GitHub's
private vulnerability reporting instead: on the repository's **Security** tab,
choose **Report a vulnerability**. Include the Narrivy version, your operating
system and the steps to reproduce. You will get an answer there.
