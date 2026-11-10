# Failure Injection

Required scenarios over time:
- kill daemon while ducking audio;
- kill model worker during inference;
- disconnect microphone mid-session;
- output backend returns error/timeout;
- compositor/session disappears;
- suspend/resume during idle/listening;
- corrupt model cache;
- disk full during model download/history write;
- IPC client disconnect;
- remote provider timeout;
- privacy policy changes mid-session;
- overlay process crashes.

Verify recovery, no stuck audio state, no duplicate text, and no silent data loss.
