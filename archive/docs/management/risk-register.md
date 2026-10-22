# Risk Register

| Risk | Impact | Mitigation |
|---|---|---|
| Wayland text insertion varies by compositor/app | High | capability router, per-app health, multiple safe fallbacks, strong diagnostics |
| Global shortcut permissions differ | High | portal/native/compositor path before evdev; capability onboarding |
| Native ML runtimes crash/hang | High | cancellation/timeouts; optional worker isolation; session recovery |
| Feature scope overwhelms 1.0 | High | phase gates/non-goals; reliability-first prioritization |
| UI becomes settings-heavy | High | outcome-oriented IA, progressive disclosure, design-system governance |
| Context intelligence leaks sensitive data | Critical | Context Fabric + Privacy Policy + zones + visible boundary |
| Personal learning becomes polluted | Medium | scopes, confidence, decay, inspect/clear controls |
| Model dependency/licensing conflicts | High | manifest provenance/license review; separate model licensing |
| Linux-first code blocks ports | High | portable compile CI + adapter boundaries |
| Performance optimized for model benchmark only | High | end-to-end latency budgets |
| Data migrations lose history/settings | Critical | versioned migrations, backups, failure tests |
| Plugin ecosystem expands attack surface | Critical | permissions, sandbox where feasible, signing/trust tiers |
| Team features complicate consumer product | Medium | separate entitlements/policy layer; simple default UI |
