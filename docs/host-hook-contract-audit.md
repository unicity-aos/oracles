# Oracle host-hook contract audit

Checked 2026-10-02 against Oracle main `670c88758464ee4ac9f7335671e5ee1d92dc0f55`
and AOS main `021d7ec445a24f1aa45d0409787b4504d63504c5`.
This is an implementation audit, not certification of a live client session.

## Sources

- [Codex hooks](https://learn.chatgpt.com/docs/hooks): event inventory, trust,
  tool coverage, output schemas, timeout limits, and unsupported outputs.
- [Claude hooks](https://code.claude.com/docs/en/hooks): event inventory,
  merging, decision control, and passive versus blocking events.
- [Grok hook contract](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-pager/docs/user-guide/10-hooks.md):
  source-pinned event list, camelCase input, blocking responses, and limitations.

Documentation describes current upstream behavior, not a minimum supported
client version. Older installed clients still require compatibility testing.

## Findings

| Host | Registered events at the audited Oracle revision | Actual response path |
| --- | --- | --- |
| Codex | SessionStart, UserPromptSubmit, PreToolUse, PermissionRequest, PostToolUse, PreCompact, PostCompact, SubagentStart, SubagentStop, Stop, SessionEnd | Wrapper emits additional context only for startup/prompt. Pre-tool and permission results are discarded; registration is not enforcement. |
| Claude | SessionStart, UserPromptSubmit | No registered PreToolUse. Prompt output is wrapped as additional context, not a blocking verdict. |
| Grok | SessionStart, UserPromptSubmit | No registered PreToolUse. Current upstream explicitly discards additionalContext from an allowing UserPromptSubmit, so context-output parity with Claude is not valid. |

AOS's hook CLI returns context only, and the MCP relay's typed response does
not preserve a decision field. Fixing a shell wrapper alone cannot complete the
policy path. Required policy replies must be authenticated, collected, retained
through the relay, and translated into the receiving host's supported output.

Codewall's managed Claude hooks are an independent direct-gate path. They do not
by themselves prove that Oracle plugin hooks are suppressed: hook sources are
merged, and Codewall's merge code preserves unrelated entries. Do not remove
the existing gate until the replacement proves denial end to end. In particular,
a protected service with another UID/runtime is not interchangeable with an
ordinary user-owned Oracle runtime.

## Lifecycle defects repaired in this branch

- Claude/Grok `Stop` no longer removes the session's route token. Only
  `SessionEnd` retires it.
- Native session IDs, including Grok's camelCase `sessionId`, are parsed without
  optional jq and mapped using their exact string digest. Punctuation cannot
  conflate sessions. Missing identity refuses rather than guessing from a PID;
  explicit valid `ASTRID_SESSION_ID` is preserved unchanged.
- Companion AOS change must map Grok `Stop` to turn completion (`message_sent`),
  not `session_end`; otherwise the server still retires the route.

`scripts/test_host_hook_lifecycle.py` runs the real wrappers with a recording AOS
fixture and a disposable application home. It checks startup -> stop -> next
prompt -> session end with interleaved punctuation-distinct sessions and no jq,
exact host session identity, token continuity, and token retirement. This proves
the wrapper contract without a Claude subscription; it
does not prove execution inside Claude or a live policy capsule.

## New or unregistered upstream events

Missing registration is not automatically a bug. Some events are optional
observations; others replace native behavior and must not be enabled casually.

- Codex: `Interrupt` is missing. It is advisory, not session termination. The
  registered SessionEnd timeout is 15 seconds, while current Codex documents a
  maximum of 3 seconds; end-to-end teardown delivery needs that smaller budget.
- Claude: besides the missing tool/lifecycle coverage above, current upstream
  documents Setup, UserPromptExpansion, PermissionDenied, PostToolUseFailure,
  PostToolBatch, Notification, MessageDisplay, TaskCreated, TaskCompleted,
  StopFailure, TeammateIdle, InstructionsLoaded, ConfigChange, CwdChanged,
  DirectoryAdded, FileChanged, WorktreeCreate, WorktreeRemove, PreModelSwitch,
  PostModelSwitch, Elicitation, and ElicitationResult. Each needs an explicit
  mapping/disposition before registration. WorktreeCreate replaces host worktree
  creation; elicitation can contain sensitive input. Neither is a safe generic
  telemetry subscription.
- Grok: current upstream also documents PostToolUseFailure, PermissionDenied,
  StopFailure, StopCancelled, Notification, SubagentStart/Stop, PreCompact,
  PostCompact, and SessionEnd. StopCancelled is distinct from successful Stop.
  PermissionRequest is not listed as a Grok event; do not copy Codex's list.

## Host-specific decision constraints

- Codex PreToolUse accepts deny but currently rejects `ask` as unsupported and
  continues the call after reporting the hook error. Never forward a generic
  policy `ask` unchanged. PermissionRequest has its own nested behavior schema.
- Claude PreToolUse supports allow/deny/ask; prompt rejection uses a different
  top-level block schema. No objection must leave native permission checks intact,
  not force an explicit allow.
- Grok settings-file PreToolUse recognizes explicit deny and ask, including
  `hookSpecificOutput.permissionDecision`. SDK-registered hooks do not support
  ask; this plugin uses settings-file commands, not that SDK registration path.
  Ordinary crashes/timeouts are
  fail-open. Prompt blocking exists but only for user-typed prompts; automatic
  wakeups and subagents are observe-only. An allowing prompt's context output
  is discarded by current upstream.
- Codex hosted tools and some specialized paths bypass ordinary tool hooks;
  write_stdin does not repeat PreToolUse for an existing process. None of these
  integrations establishes control over every operation the host can perform.

## Remaining completion evidence

### Ownership and compatibility

Oracle owns host-specific hook registration, event translation and native
responses. AOS supplies generic runtime/principal/service mechanisms; it must
not learn Claude, Codex or Grok settings formats. Codewall registers its policy
on the authenticated bus rather than owning the host integration.

The CLI now exposes `aos hook --format json`: an authenticated envelope with
`schema_version`, exact `event`, neutral `decision` and optional `context`.
Oracle's `aos-native-hook` translates that envelope into each host's response.
Empty output, a missing decision, wrong event, or malformed envelope refuses
the binding request; it is not interpreted as permission. The existing default
context output remains unchanged. The earlier unpublished `--format native`
candidate interface has been replaced rather than shipped into AOS.

Personal plugin operation and administrator-protected operation are distinct.
Existing protected installations must not silently become user-disableable
plugin installations. A protected handoff must preserve executable/configuration
custody, the cross-account evaluation boundary, the selected principal and the
installation identity. A normal same-user bus test does not establish this.

Migration must verify the replacement before selectively removing an owned
legacy registration. Failure retains existing protection and unrelated settings.
Recovery must be available outside the blocked host; an arbitrary command
allowlist is not a substitute for that recovery path.

Codewall's ordinary repair path avoids implicitly provisioning the legacy
Claude managed gate. Existing managed bindings require the explicit protected
repair path. The companion installer now requests a protected-helper export
during verified Oracle installation, captures the export manifest digest and
passes that selection to the privileged handoff. These are candidate changes,
not a statement about an already-published installer.

The companion Codewall `oracle_policy_lifecycle_probe.py` now exercises the
registered commands against packaged adapter, MCP and enforcer capsules in an
isolated runtime. Its twelve scenarios per host cover unregistered responders,
required allow/ask, malformed and silent replies, invalid effective policy
configuration, unregister with the adapter absent, restoration without stale
policy requirements, immediate/delayed optional context, and reinstall with both
forbidden and benign inputs.
A separate signed test publisher supplies policy activation and competing
responses; this is not a production enrollment or an interactive host test.
Codex's unsupported pre-tool ask is deliberately translated to deny.

This bus test does not prove the protected-service handoff or behavior inside a
subscribed Claude client. Those are separate verification boundaries.

For each supported host: test allowed, denied, malformed, unavailable, and late
policy responses through the registered command; assert exact session/principal
isolation and host-native output. Then exercise the real adapter + policy capsule
with a disposable runtime. Keep fixture tests, live bus tests, and live host
tests separately labeled. Do not remove the managed gate or claim migration
complete based on this audit or lifecycle regression alone.

### Protected evaluator adapter (not yet activated)

`aos-protected-hook` owns native input normalization and response rendering. It
calls `codewall-gate --format oracle-json` with explicit service socket, service
UID and installation identity; it neither discovers nor changes those pins.
The evaluator retains peer authentication, service-owned principal, audit
commit/signing and its worker supervisor. Native payload principal/session
fields cannot select a protected policy. Grok camelCase tool fields are
normalized and truncated/conflicting inputs refuse evaluation.

The adapter and its sibling `aos-native-hook` must be deployed together in
administrator-protected storage and invoked with a protected Python interpreter
using `-I`. Merely copying them into a mutable plugin cache does not satisfy the
protected installation contract. Existing managed registrations are untouched
while that deployment/migration is incomplete. A missing codec exits 2 rather
than the nonblocking exit 1 used by Claude for ordinary hook failures.

`scripts/test_protected_hook_delivery.py` exercises executable fixture
evaluators for native allow/no-objection, deny, ask, unavailable, malformed and
timeout responses. It is not evidence of service custody or a subscribed host
application. Protected registration remains established only for the legacy
Claude deployment; codec support for Codex/Grok is not a claim of equivalent
administrator-enforced registration on those hosts.

`aos-protected-settings` is the non-mutating handoff planner. It matches the
exact legacy binding for both Claude enforcement events, preserving either
service custody or explicit local runtime/journal custody and unrelated hooks.
`aos-protected-deploy` stages verified immutable adapter/codec/evaluator bytes,
checks readiness under the supervised UID and compares/publishes settings under
the stable settings lock. Failure before publication leaves old settings intact.
Retained transactions support exact removal and executable/source refresh.

Custody changes are explicit: Codewall's privileged caller validates its
existing committed re-enrolment or qualified service-rotation intent before
requesting an Oracle transaction. Oracle renders the permitted journal pair or
service socket/installation pair change; principal, supervised UID and other
stable identity fields cannot change through that operation. The planner does
not authenticate Codewall intent records itself. Calling it directly is not
proof of authorization. The deployer remains administrator-only.

### Candidate verification boundaries (2026-10-03)

- Registered plugin commands: the 36-case packaged lifecycle test passed again
  across three separate host principals. This exercised the actual AOS relay,
  adapter, MCP and Codewall enforcer with a signed test-policy publisher.
- Real local evaluator: `oracle_native_gate_probe.py` passed 24 cases through
  the actual Codewall gate and packaged enforcer, rendered by Oracle's protected
  adapter for all three host formats. Benign inputs produced no objection;
  matching inputs returned the actual rule denial; wrong sources and malformed
  input refused. This was same-user local evaluation, not cross-UID service use.
- Deployment mechanics: `scripts/test_protected_deploy_root.py` passed in a
  disposable, network-disabled Linux container with real root-owned files,
  non-root refusal and actual UID switching. Migration, failed-readiness
  preservation, explicit rotation, retry and removal passed. Its evaluator is
  synthetic; it does not prove real protected-service policy evaluation.
- The Python unit transaction tests mock privilege/readiness explicitly. They
  remain useful regression tests but are not substituted for the container run.
- Public privileged CLI: Codewall's `oracle_native_deploy_root.py` passed with
  the actual Linux Rust deployment binary and real root/nobody identities.
  Manifest-digest refusal, migration/retry, retained-record check/repair,
  failed-readiness preservation and removal passed. The export manifest and
  evaluator are fixtures, not proof of signed downloads or real service policy.
- Live Claude/Codex/Grok application sessions and a fully assembled protected
  service migration have not been verified by these tests. Independent review
  and final integration evidence remain required before claiming completion.
