# UI String Guidelines

- concise, concrete, actionable;
- avoid runtime jargon in primary messages;
- do not embed punctuation/whitespace assumptions outside translation messages;
- parameters are typed/contextual (`{microphoneName}`, `{modelName}`, `{seconds}`), never positional mystery values;
- permission errors say what capability is affected and what action can fix it;
- never interpolate transcript/clipboard/context content into ordinary errors/loggable messages;
- translators receive comments for ambiguous terms such as Mode, Profile, Inbox, Raw, Clean, processing boundary, and push-to-talk.
