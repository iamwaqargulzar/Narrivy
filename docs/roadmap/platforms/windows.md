# Windows Portability Contract

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

Future adapter likely maps:
- audio → WASAPI;
- app/selection/context → Win32/UI Automation;
- text delivery → UI Automation and/or SendInput where appropriate;
- shortcut → RegisterHotKey/hooks as needed;
- secret store → Windows Credential Manager;
- background lifecycle → user background process/service mechanisms;
- compute → CPU/CUDA/DirectML/OpenVINO.

Portable crates must compile on Windows before this adapter is feature complete.
