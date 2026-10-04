# Native hook coverage

`plugins/common/bin/aos-native-hook` owns the native inventory and codec.
`python3 scripts/sync-hook-registrations.py` generates all three registration
files; `--check` fails on drift. The common helper is vendored by
`scripts/sync-plugins.sh`, so installed plugins are self-contained.

Registrations cover 31 Claude Code, 12 Codex, and 15 Grok events from the
current documented surfaces. Availability still depends on the installed
client version; registering an event does not make an old client emit it.

All PreToolUse callbacks become `hook.v1.event.before_tool_call`. Native fields
are retained, and AOS adds common snake-case tool fields. Capsules use the
kernel-stamped principal and correlation; native payload identity cannot
select another user's policy. Astrid's kernel does not know these clients.

The codec distinguishes native permission decisions, blocking feedback,
compaction continuation, exit-code decisions, context, and observations.
Post-tool/Stop feedback cannot undo a completed tool operation. Codex does not
support PreToolUse `ask`, so that result refuses rather than disappearing.
Observational callbacks never become a fictional block. Missing transport
refuses decision-capable callbacks and reports undelivered observations on
stderr. TeammateIdle and TaskCompleted refusals use exit 2 plus stderr.

Observer/context delivery has a two-second internal deadline, below the
three-second registration limit. Binding delivery has ten seconds below its
fifteen-second registration limit. Hooks do not provision or install capsules;
SessionStart's separate setup helper owns readiness. Explicit SessionEnd
retires the route, while Grok child teardown keeps its parent route. Repeated
Stop callbacks preserve the client's recursion marker for AOS to avoid loops.

Elicitation can be declined without inventing submitted content. Entered
ElicitationResult `content` is redacted before hook transport, with a second
redaction at canonical publication for callers using the AOS CLI directly.
This is not a general transcript or arbitrary-payload secret scrubber.

Two Claude hooks are intentionally not registered: WorktreeCreate and
WorktreeRemove replace filesystem lifecycle operations. Registering a passive
helper there would break creation or falsely report cleanup. Native ownership
is retained. FileChanged has no wildcard matcher: it observes the client's
existing watch set, not the entire filesystem. MessageDisplay is observed,
not rewritten. Setup is observed, not used to request installation.

Required-policy configuration lives in AOS's hook adapter. Legacy policy
registrations still protect prompts/pre-tool; new events use the explicit
canonical-event policy map. Existing Codewall responders are not required to
answer unrelated hooks. Updating registrations does not publish an Oracle
release, activate a plugin, or prove execution inside a live host account.
Ship the expanded AOS adapter before this Oracle registration change: older
adapters do not implement these decision events, and an unavailable binding
reply intentionally refuses rather than pretending enforcement succeeded.

Upstream references:

- [Claude Code](https://code.claude.com/docs/en/hooks)
- [Codex](https://learn.chatgpt.com/docs/hooks)
- [Grok](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/10-hooks.md)
