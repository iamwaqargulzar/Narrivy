# Context Fabric

> **Status: roadmap / not in 1.0.** This describes a planned design, not current behaviour. See [the roadmap](../README.md).

Context is a permissioned set of typed facts, not an unstructured global prompt.

Providers may expose:
- active app and window;
- target control metadata;
- selected text;
- limited surrounding text;
- clipboard;
- repository/project metadata;
- indexed project vocabulary/symbols;
- personal/scoped dictionaries;
- optional calendar/contact/meeting facts;
- conversation/session-local context.

Each context item carries:
- source;
- timestamp/freshness;
- sensitivity class;
- scope;
- retention rule;
- whether remote processing is allowed.

Modes request specific context capabilities. Privacy Policy evaluates the request before collection and again before any remote boundary.
