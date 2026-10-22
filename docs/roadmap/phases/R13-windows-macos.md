# R13 — Native Windows & macOS Proof (Post-1.0)

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

Build native platform adapters/clients against the existing portable core/protocol contracts. Do not fork domain behavior into platform-specific products. Prove shortcut, audio, target/text delivery, IPC/secret store, packaging, accessibility and failure semantics using native OS APIs and capability-driven UI. Any contract that cannot support a native platform should be evolved compatibly with migration/version tests rather than bypassed.
