# Linux Packaging Workspace

P7 places maintained packaging sources here.

Expected structure after implementation:

```text
packaging/linux/
├── deb/   # the single 1.0 installation artifact (owner decision 2026-08-22)
├── desktop/
├── icons/
├── scripts/
└── tests/
```

Do not commit opaque final binaries here; release binaries belong under `artifacts/releases/` and are normally produced by release CI/local release tooling. Packaging sources and metadata are version-controlled.

The package must start/use a per-user daemon according to the selected lifecycle design, not a privileged system daemon unless a later ADR proves that necessary.
