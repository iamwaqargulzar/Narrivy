# Owner-authorized Git history reset — 2026-09-05

The owner authorized the proposed reset with an external backup. Original Git
metadata (objects, refs, reflogs, index and configuration) was moved intact to
`../narrivy-git-history-backup-20260905T040120Z/git`. The backup also records
pre-reset file hashes, working-tree status, original branch and original HEAD.

A new source baseline replaces local history. Current working files, including
uncommitted recovery changes, are retained. Local author identity, master branch
name and origin configuration are preserved. Nothing is pushed. Original HEAD
was `ff6d864012adb1468d02c6b5f80b9ce92258fa04`.

Generated target/dependency trees remain excluded. Historical Debian packages
remain on disk, now ignored under artifacts/releases; they are also recoverable
from the external Git backup. No build cache, model, package or vendor tree was
deleted. Moving the backup outside this folder does not reclaim total disk space.

Historical evidence and last_gate_commit retain their original identities and
remain quarantined. No requirement, phase or release acceptance is transferred
to the new baseline. No manifest is resealed. The reset is repository maintenance,
not a release qualification or the owner's authorization to publish.
