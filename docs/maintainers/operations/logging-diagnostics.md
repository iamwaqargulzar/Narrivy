# Logging and Diagnostics

Use structured events with subsystem, event code, severity, session correlation ID and timing — but no transcript/audio/context content by default.

Diagnostics should expose:
- app version/build ID;
- distro/OS/kernel;
- desktop/session/compositor;
- shortcut backend;
- text delivery backend + health;
- PipeWire/audio device summary;
- compute devices/runtimes;
- model/runtime versions;
- cache status;
- recent content-free error codes;
- latency percentiles.

“Copy Support Report” must clearly preview what is included.
