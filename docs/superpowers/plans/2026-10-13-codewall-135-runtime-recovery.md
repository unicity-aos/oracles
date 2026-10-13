# Stopped-runtime hook recovery implementation plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Recover Claude's blocking native hooks after the per-user AOS daemon stops, within the existing deadline and without weakening policy enforcement.

**Architecture:** The native wrapper requests runtime readiness from its existing `aos-up` launcher before binding event delivery. The launcher resolves the same AOS executable and workspace, accepts only a valid running/stopped status, and starts a stopped runtime through `aos --principal default start`. It delivers the event once, after readiness preparation. The existing ten-second wrapper deadline covers status, start, and delivery.

**Tech Stack:** Python standard library, POSIX shell, existing AOS CLI.

**Spec:** Codewall issue #135 investigation and the user's instruction to implement recovery; the design is recorded above. No new services, policy bypass, provisioning, or retries of delivered events. Passive hooks and other hosts retain their current lifecycle behavior.

## Global constraints

- Use the selected AOS CLI, never standalone Astrid. Enter the product runtime workspace.
- Start only on a successful status response whose state is `stopped`; unknown, invalid, or failed status refuses delivery.
- Keep the existing ten-second binding-hook deadline and three-second registered observation budget.
- Startup receives no hook input. Startup diagnostics never enter the JSON decision stream.
- Preserve authenticated hook output validation and denials.
- Source edits belong in common helpers; synchronize packaged copies.

## Review focus

- Already-running runtime: no start; one event.
- Stopped runtime: successful recovery preserves both allow and deny replies.
- Failed/malformed/unknown status and startup that leaves the runtime stopped: explicit refusal; no event.
- Hung startup and detached child: bounded refusal and no inherited-pipe delay.
- Other hosts, passive hooks, invalid input, and wrong principal: no new startup side effects.

### Task 1: Recover before Claude binding-hook delivery

**Files:** `scripts/test_native_hook_delivery.py`, `plugins/common/bin/aos-native-hook`, `plugins/common/bin/aos-up`, their synchronized packaged copies.

**Interfaces:** Internal launcher invocation `aos-up hook <event> --ensure-runtime`. The native wrapper requests this only for Claude binding modes. `emit_host_hook` consumes the flag before invoking AOS's authenticated hook command.

- [x] Add subprocess regression fixtures with an isolated AOS home and a recording AOS executable. Assert stopped-to-running recovery, preserved policy deny, running fast path, safe failures, one delivery, no provisioning, product cwd, and unchanged passive/other-host behavior.
- [x] Run `python3 scripts/test_native_hook_delivery.py` and observe new recovery tests fail against the old code.
- [x] Add status/start preparation under the existing wrapper timeout; update failure guidance to the concrete AOS start command; sync packaged helpers.
- [x] Run the native suite and related host startup/lifecycle tests; verify syntax and packaging sync.

### Task 2: Verify and document the recovery contract

**Files:** `docs/native-hook-inventory.md` and this plan's progress section.

- [x] Document bounded cold-runtime recovery and manual remediation.
- [x] Run relevant plugin CI suites and project workspace tests where supported. Report environment limitations and unrelated failures without expanding scope.
- [x] Review the complete diff with a fresh reviewer, fix material findings, and preserve the tested patch in the task worktree.

## Progress

- Baseline native-hook suite: 14 tests passed before edits.
- Pre-flight: tasks share only the recovery contract; no conflicting interfaces.
- Ruling: use native execution, then commit and publish the reviewed patch as requested.
- RED: the eight new regression tests produced nine failing assertions/subtests against the original code (the passive-hook check already passed). Failures exposed the missing startup, status guard, and remediation message.
- GREEN: all 22 native-hook tests passed, including the ten-second startup timeout and a detached process inheriting startup streams.
- Passed: host MCP startup, host-hook lifecycle, hook-adapter binding, protected-hook delivery, protected settings, protected asset export, preflight notices, MCP setup status, Codex catalog, Codex MCP startup, and installer reuse suites.
- Passed: `cargo test --workspace --locked --target aarch64-apple-darwin`; ShellCheck for the modified launchers; hook-registration drift check; `git diff --check`.
- Baseline limitation: `scripts/test_protected_deploy.py` reports eight permission errors while renaming staged generations on this macOS host. Reproduced the same eight errors on untouched `08346e7`; no unrelated fix attempted. Affected tests: `test_asset_mismatch_or_modified_generation_fails_closed`, `test_commit_and_retry_preserve_unrelated_settings_and_baseline`, `test_failed_readiness_leaves_old_enforcement_unchanged`, `test_intervening_edit_is_never_overwritten`, `test_removal_is_exact_retryable_and_does_not_require_a_running_policy`, `test_removal_preserves_intervening_administrator_changes`, `test_repair_checks_readiness_before_replacing_and_can_retry_or_remove`, `test_root_requested_rotation_preserves_old_hooks_until_ready_and_retries`.
- Validation limit: the AOS CLI is a recording executable in the recovery integration tests. Real reboot recovery on the affected Linux installation has not been exercised. No release, deployment, or live user configuration changes were performed.
- Review corrections: reject duplicate status fields and confirm runtime state after every start attempt. A failed start can recover if another launcher made the runtime running; a successful start that leaves it stopped cannot deliver. Three additional regressions failed before these changes and passed afterward.
- Final verification: 25 native-hook tests passed; host MCP startup, host-hook lifecycle, ShellCheck, registration sync, and diff whitespace checks passed again after the review corrections. The reviewer found no remaining material code issue.
- Complete: implementation and review finished on `fix/codewall-135-runtime-recovery`. The original checkout and live runtime were not modified. Temporary read-only inspection and baseline worktrees were removed.

## Binding-hook lifetime correction

Real source-composition testing found that successful cold/warm delivery did not suffice: an invalid token was correctly refused, but the next valid owner was refused while an ephemeral daemon finalized, and immediate stop returned a durable projection fence error. This was distinct from invalid diagnostic reuse of a session ID with a fresh token. Original failed evidence remains retained.

Recovery now starts AOS in its normal persistent mode. Binding calls may have no lasting MCP connection; their runtime must survive gaps between prompts. This introduces no login service, policy bypass, retry of delivered events, or change to the ten-second deadline. Existing running daemons are left alone, and explicit operator stop still ends the recovered runtime. Passive hooks and other-host native paths retain their lifecycle.

The recording startup regression rejects the old ephemeral command. Real qualification requires a fresh session, cold and warm success, persistent status, wrong-token refusal, valid-owner recovery, route retirement and explicit stopped/volume-only cleanup. Source composition remains distinct from published packages and physical reboot acceptance.
