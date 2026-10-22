# Autonomous Execution Loop

Narrivy's build pack is designed for coding agents that may work across many context windows/runs.

## Durable state

`agent/STATE.json` is the current pointer. The codebase/tests/evidence are the source of truth; state is a resumable index, not a substitute for verification.

At the start of each work session:
1. read state/active phase;
2. inspect version-control status and recent commits;
3. run doctor and the last known relevant gate;
4. inspect blockers/decisions/research;
5. resume the smallest failing or incomplete work item.

At each meaningful checkpoint:
- commit a coherent change when version control is available;
- record decision/research only when durable/useful;
- update `current_work_item`/gate result;
- keep generated evidence out of source directories unless it is specification/reference material.

## Phase advancement

A phase advances only when:
- implementation requirements are satisfied;
- acceptance spec contains real commands/artifacts (`implemented:true`);
- primary traceability requirements are passed with evidence;
- phase gate exits zero;
- prior working E2E remains passing from P6 onward.

Then update state and immediately continue to the next phase.

## Context loss recovery

Never rely on hidden chat context. Durable decisions belong in repository files. If a future agent instance knows nothing except this ZIP/repository, it must be able to resume from state, tests, ADRs, evidence and version history.
