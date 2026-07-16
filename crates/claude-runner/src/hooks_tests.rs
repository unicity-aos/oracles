//! Host-side unit tests for items declared in `hooks.rs`.
//!
//! Loaded via `#[path = "hooks_tests.rs"] mod tests;` from `hooks.rs`,
//! mirroring the `lib.rs` / `lib_tests.rs` convention. Keeps the
//! production module short while preserving `super::*` access to
//! crate-private items.

use super::*;

#[test]
fn hook_token_key_uses_prefix_and_ids() {
    let key = hook_token_key("alice", "sess-123");
    assert_eq!(key, "claude.hook_token.alice.sess-123");
}

#[test]
fn hex_encode_round_trip_known_vectors() {
    assert_eq!(hex_encode(&[]), "");
    assert_eq!(hex_encode(&[0x00]), "00");
    assert_eq!(hex_encode(&[0xff]), "ff");
    assert_eq!(hex_encode(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
}

#[test]
fn tokens_match_equal_inputs_true() {
    assert!(tokens_match("deadbeef", "deadbeef"));
    assert!(tokens_match("", ""));
}

#[test]
fn tokens_match_unequal_inputs_false() {
    assert!(!tokens_match("deadbeef", "deadbeee"));
    assert!(!tokens_match("deadbeef", "00000000"));
}

#[test]
fn tokens_match_length_mismatch_false() {
    assert!(!tokens_match("dead", "deadbeef"));
    assert!(!tokens_match("deadbeef", "dead"));
    assert!(!tokens_match("", "x"));
}

#[test]
fn topic_map_contains_all_known_hooks() {
    let names: Vec<&str> = HOOK_TOPIC_MAP.iter().map(|(n, _)| *n).collect();
    for expected in [
        "session_start",
        "session_end",
        "session_setup",
        "message_received",
        "message_expanded",
        "before_tool_call",
        "after_tool_call",
        "after_tool_call_failed",
        "after_tool_batch",
        "permission_requested",
        "permission_denied",
        "message_sent",
        "message_failed",
        "subagent_start",
        "subagent_stop",
        "task_created",
        "task_completed",
        "teammate_idle",
        "on_compaction_started",
        "on_compaction_completed",
        "config_changed",
        "instructions_loaded",
        "file_changed",
        "cwd_changed",
        "worktree_created",
        "worktree_removed",
        "elicitation_requested",
        "elicitation_resolved",
        "message_displayed",
        "notification",
    ] {
        assert!(names.contains(&expected), "missing hook tail {expected}");
    }
    assert_eq!(names.len(), 30);
}

#[test]
fn topic_map_notification_uses_claude_namespace() {
    let target = HOOK_TOPIC_MAP
        .iter()
        .find(|(n, _)| *n == "notification")
        .map(|(_, t)| *t)
        .unwrap();
    assert_eq!(target, "claude.v1.notification");
}

#[test]
fn topic_map_canonical_entries_use_hook_v1_event_prefix() {
    for (name, topic) in HOOK_TOPIC_MAP {
        if *name == "notification" {
            continue;
        }
        assert!(
            topic.starts_with("hook.v1.event."),
            "non-notification entry {name} -> {topic} missing canonical prefix"
        );
    }
}

#[test]
fn canonical_topic_for_resolves_all_known_hooks() {
    assert_eq!(
        canonical_topic_for("session_start"),
        Some("hook.v1.event.session_start")
    );
    assert_eq!(
        canonical_topic_for("session_end"),
        Some("hook.v1.event.session_end")
    );
    assert_eq!(
        canonical_topic_for("before_tool_call"),
        Some("hook.v1.event.before_tool_call")
    );
    assert_eq!(
        canonical_topic_for("after_tool_call"),
        Some("hook.v1.event.after_tool_call")
    );
    assert_eq!(
        canonical_topic_for("message_sent"),
        Some("hook.v1.event.message_sent")
    );
    assert_eq!(
        canonical_topic_for("subagent_start"),
        Some("hook.v1.event.subagent_start")
    );
    assert_eq!(
        canonical_topic_for("on_compaction_started"),
        Some("hook.v1.event.on_compaction_started")
    );
    assert_eq!(
        canonical_topic_for("on_compaction_completed"),
        Some("hook.v1.event.on_compaction_completed")
    );
    assert_eq!(
        canonical_topic_for("notification"),
        Some("claude.v1.notification")
    );
    assert_eq!(canonical_topic_for("not_a_real_hook"), None);
}

#[test]
fn envelope_parses_minimal_shape() {
    // Canonical envelope shape published by `astrid-emit` once the
    // core PR for astrid-runtime/astrid#814 lands.
    let raw = r#"{
        "hook": "before_tool_call",
        "payload": "eyJ0b29sIjoiZnMifQ==",
        "correlation_id": null,
        "principal_id": "alice",
        "session_id": "sess-1",
        "token": "deadbeef"
    }"#;
    let e: HookEnvelope = serde_json::from_str(raw).unwrap();
    assert_eq!(e.hook, "before_tool_call");
    assert_eq!(e.payload, "eyJ0b29sIjoiZnMifQ==");
    assert_eq!(e.principal_id, "alice");
    assert_eq!(e.session_id, "sess-1");
    assert_eq!(e.token, "deadbeef");
    assert!(e.correlation_id.is_none());
}

#[test]
fn envelope_treats_missing_correlation_id_as_none() {
    // `correlation_id` is `#[serde(default)]` so producers MAY
    // omit the key entirely (in addition to emitting it as
    // `null`). Both forms round-trip to `None`.
    let raw = r#"{
        "hook": "after_tool_call",
        "payload": "",
        "principal_id": "bob",
        "session_id": "sess-2",
        "token": "cafe"
    }"#;
    let e: HookEnvelope = serde_json::from_str(raw).unwrap();
    assert!(e.correlation_id.is_none());
}

/// Helper: a canonical envelope used by the `build_canonical_body`
/// regression suite. Sets every field including the transport ones
/// (`session_id`, `token`) so the strip assertions have something
/// to assert against.
fn envelope_for_body_test() -> HookEnvelope {
    HookEnvelope {
        hook: "before_tool_call".to_string(),
        payload: "eyJ0b29sIjoiZnMifQ==".to_string(),
        correlation_id: Some("corr-1".to_string()),
        principal_id: "alice".to_string(),
        session_id: "sess-1".to_string(),
        token: "deadbeef".to_string(),
    }
}

/// CORE STRIP-THE-TRANSPORT REGRESSION.
///
/// The canonical body MUST NOT carry `session_id` or `token` in the
/// serialized JSON. Those are transport-layer fields the validator
/// uses to authenticate the producer; subscribers on
/// `hook.v1.event.<name>` must never see them. A regression that
/// adds them back (e.g. accidentally serialising the entire
/// envelope) would silently leak the per-session secret onto a
/// public-ish topic.
///
/// The assertion is performed on the *serialized* JSON string so a
/// `#[serde(skip_serializing)]` annotation that's later removed
/// would still be caught — the field absence is asserted on the
/// wire form, not just the Rust `Value`.
#[test]
fn canonical_body_strips_session_id_and_token() {
    let env = envelope_for_body_test();
    let body = build_canonical_body(&env);
    let wire = serde_json::to_string(&body).unwrap();
    assert!(
        !wire.contains("session_id"),
        "canonical body leaked session_id: {wire}",
    );
    assert!(
        !wire.contains("token"),
        "canonical body leaked token bytes: {wire}",
    );
    // Belt-and-braces on the raw token value too -- a refactor
    // that renames the JSON key but still embeds the secret would
    // be caught by this check alone.
    assert!(
        !wire.contains("deadbeef"),
        "canonical body leaked the secret token value: {wire}",
    );
}

/// The canonical body MUST preserve `principal_id` -- per the
/// workflow brief, the runner attributes the republish from its own
/// capsule and the principal claim has no other channel onto the
/// wire.
#[test]
fn canonical_body_keeps_principal_id_inside_payload() {
    let env = envelope_for_body_test();
    let body = build_canonical_body(&env);
    assert_eq!(
        body.get("principal_id").and_then(|v| v.as_str()),
        Some("alice"),
    );
}

/// The canonical body's `hook` field must equal the envelope's --
/// downstream subscribers route on this even when subscribed via
/// the wildcard `hook.v1.event.*`.
#[test]
fn canonical_body_preserves_hook_field() {
    let env = envelope_for_body_test();
    let body = build_canonical_body(&env);
    assert_eq!(
        body.get("hook").and_then(|v| v.as_str()),
        Some("before_tool_call"),
    );
}

/// `payload` is the opaque base64 blob Claude's hook produced and
/// must round-trip byte-for-byte: a subscriber that base64-decodes
/// it is reading the original Claude-side JSON.
#[test]
fn canonical_body_preserves_payload_bytes() {
    let env = envelope_for_body_test();
    let body = build_canonical_body(&env);
    assert_eq!(
        body.get("payload").and_then(|v| v.as_str()),
        Some("eyJ0b29sIjoiZnMifQ=="),
    );
}

/// `correlation_id: None` MUST serialize as JSON `null` (not be
/// omitted) so downstream consumers can rely on the key being
/// present regardless of whether the producer set it.
#[test]
fn canonical_body_serializes_none_correlation_as_null() {
    let env = HookEnvelope {
        correlation_id: None,
        ..envelope_for_body_test()
    };
    let body = build_canonical_body(&env);
    let wire = serde_json::to_string(&body).unwrap();
    assert!(
        wire.contains("\"correlation_id\":null"),
        "expected null correlation_id in wire form: {wire}",
    );
}

/// `correlation_id: Some(...)` MUST round-trip as the original
/// string so a tool-call response can be correlated back to the
/// triggering hook fire.
#[test]
fn canonical_body_preserves_some_correlation_id() {
    let env = envelope_for_body_test();
    let body = build_canonical_body(&env);
    assert_eq!(
        body.get("correlation_id").and_then(|v| v.as_str()),
        Some("corr-1"),
    );
}

/// Exactly four top-level keys: `hook`, `payload`, `correlation_id`,
/// `principal_id`. Anything else is either a transport leak (bad)
/// or new contract surface that needs a deliberate doc-comment +
/// downstream-consumer update.
#[test]
fn canonical_body_has_exactly_four_keys() {
    let env = envelope_for_body_test();
    let body = build_canonical_body(&env);
    let obj = body.as_object().expect("body must be a JSON object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["correlation_id", "hook", "payload", "principal_id"],
    );
}

/// Even adversarial principal_ids that include the literal token
/// value as a substring (e.g. a confused-deputy producer that
/// prefixed its own claimed principal with the token) must not
/// cause the strip-the-transport assertion to mistakenly succeed.
/// This is a defensive test for the test ITSELF -- guards against
/// the assertion accidentally over-matching.
#[test]
fn canonical_body_token_substring_check_is_not_a_false_positive() {
    let env = HookEnvelope {
        principal_id: "user-deadbeef-test".to_string(),
        token: "cafef00d".to_string(),
        ..envelope_for_body_test()
    };
    let body = build_canonical_body(&env);
    let wire = serde_json::to_string(&body).unwrap();
    // The principal_id legitimately contains "deadbeef"
    // (the *previous* token's value) -- that's fine: it's the
    // claimed principal, not a leaked token. The real test is
    // that the NEW token "cafef00d" is absent.
    assert!(wire.contains("user-deadbeef-test"));
    assert!(!wire.contains("cafef00d"));
    assert!(!wire.contains("\"token\""));
}

/// audit_truncate must pass through short inputs unchanged. Bounds
/// the audit topic's amplification factor on adversarial input --
/// but only when needed; legitimate ids (the runner's `validate_id`
/// already caps at 128 bytes) flow through untouched.
#[test]
fn audit_truncate_passes_short_strings_through() {
    assert_eq!(audit_truncate(""), "");
    assert_eq!(audit_truncate("alice"), "alice");
    let exactly_cap = "a".repeat(AUDIT_FIELD_CAP);
    assert_eq!(audit_truncate(&exactly_cap), exactly_cap);
}

/// audit_truncate caps oversized inputs at AUDIT_FIELD_CAP bytes
/// and the result is valid UTF-8 (i.e. the truncation lands on a
/// char boundary).
#[test]
fn audit_truncate_caps_long_strings_on_char_boundary() {
    // Pure ASCII -- exactly AUDIT_FIELD_CAP bytes.
    let big = "x".repeat(AUDIT_FIELD_CAP * 4);
    let truncated = audit_truncate(&big);
    assert_eq!(truncated.len(), AUDIT_FIELD_CAP);
    assert!(truncated.is_char_boundary(truncated.len()));

    // Multi-byte UTF-8 (3 bytes per char) -- must NOT slice mid-
    // codepoint. 4-byte heart emoji '\u{2764}\u{fe0f}' is two
    // codepoints (3 + 3 = 6 bytes per heart-pair); repeat enough
    // to exceed the cap.
    let utf8 = "\u{2764}\u{fe0f}".repeat(AUDIT_FIELD_CAP);
    let truncated = audit_truncate(&utf8);
    assert!(truncated.len() <= AUDIT_FIELD_CAP);
    // The slice must be valid UTF-8 -- if it isn't, this would
    // already have panicked in audit_truncate's `&s[..cap]` step.
    // Belt-and-braces: assert the result re-roundtrips via
    // from_utf8.
    assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());
}

/// The validator's mint path produces hex strings of the expected
/// length: 32 bytes -> 64 hex chars. We can't exercise
/// `mint_token` directly (requires the host CSPRNG which isn't
/// available off-target), but the underlying `hex_encode` is the
/// observable contract and we pin both the length and alphabet
/// here.
#[test]
fn hex_encode_produces_expected_length_and_alphabet() {
    let bytes = [0u8; TOKEN_BYTES];
    let encoded = hex_encode(&bytes);
    assert_eq!(encoded.len(), TOKEN_BYTES * 2);
    assert!(
        encoded.chars().all(|c| c.is_ascii_hexdigit()),
        "encoded token must be ASCII hex: {encoded}",
    );
    // Strictly lowercase hex -- case-sensitivity matters for the
    // constant-time compare against the stored value.
    assert!(
        encoded
            .chars()
            .all(|c| !c.is_ascii_alphabetic() || c.is_ascii_lowercase()),
        "encoded token must be lowercase hex: {encoded}",
    );
}

/// Tokens with identical-length-but-different-content must
/// `tokens_match == false`. Catches a regression that short-
/// circuits to `true` on length match alone.
#[test]
fn tokens_match_xor_accumulator_detects_single_byte_diff() {
    // 64-hex-char strings -- same length as a real 256-bit token --
    // differing in only the very last nibble. A naive implementation
    // that bails after the first equal pair would erroneously
    // return true.
    let a = "f".repeat(64);
    let mut b_bytes: Vec<u8> = a.bytes().collect();
    let last = b_bytes.len() - 1;
    b_bytes[last] = b'e';
    let b = String::from_utf8(b_bytes).unwrap();
    assert!(!tokens_match(&a, &b));

    // Symmetric -- same diff in the FIRST byte.
    let mut c_bytes: Vec<u8> = a.bytes().collect();
    c_bytes[0] = b'e';
    let c = String::from_utf8(c_bytes).unwrap();
    assert!(!tokens_match(&a, &c));
}

/// `hook_token_key` must round-trip through every place the runner uses
/// it: spawn (`persist_token`), shutdown (`forget_token`), and
/// run-loop (`lookup_token`). Pin the exact format so a refactor
/// that changes the separator or the prefix surfaces here.
#[test]
fn hook_token_key_format_is_stable_for_kv_lookups() {
    // Spot-check the prefix concatenation matches the constant.
    let key = hook_token_key("p1", "s1");
    assert!(key.starts_with(HOOK_TOKEN_KEY_PREFIX));
    assert!(key.contains(".p1."));
    assert!(key.ends_with(".s1"));

    // Reject empty inputs: format! happily concatenates them, but
    // that's a caller bug -- the runner's `validate_id` rejects empty
    // ids before they reach this helper. Document the assumption
    // by asserting on a real-world id alphabet.
    let realistic = hook_token_key("user-abc_123", "550e8400-e29b-41d4-a716-446655440000");
    assert_eq!(
        realistic,
        "claude.hook_token.user-abc_123.550e8400-e29b-41d4-a716-446655440000",
    );
}

/// Cross-crate sync: the topic table claude-install reads (when
/// authoring `settings.local.json`) must equal the runner's own
/// validator table byte-for-byte. Right now the table is defined
/// in both crates because there's no shared oracle-host crate; this
/// test pins the LOCAL table's shape so a drift in claude-install
/// surfaces in CI (claude-install's own test asserts the same).
#[test]
fn topic_map_matches_documented_claude_install_alphabet() {
    // The exact ordering matters -- claude-install iterates this
    // table when emitting hook command strings, and the resulting
    // settings.local.json should be deterministic across runs.
    let expected = [
        ("session_start", "hook.v1.event.session_start"),
        ("session_end", "hook.v1.event.session_end"),
        ("session_setup", "hook.v1.event.session_setup"),
        ("message_received", "hook.v1.event.message_received"),
        ("message_expanded", "hook.v1.event.message_expanded"),
        ("before_tool_call", "hook.v1.event.before_tool_call"),
        ("after_tool_call", "hook.v1.event.after_tool_call"),
        (
            "after_tool_call_failed",
            "hook.v1.event.after_tool_call_failed",
        ),
        ("after_tool_batch", "hook.v1.event.after_tool_batch"),
        ("permission_requested", "hook.v1.event.permission_requested"),
        ("permission_denied", "hook.v1.event.permission_denied"),
        ("message_sent", "hook.v1.event.message_sent"),
        ("message_failed", "hook.v1.event.message_failed"),
        ("subagent_start", "hook.v1.event.subagent_start"),
        ("subagent_stop", "hook.v1.event.subagent_stop"),
        ("task_created", "hook.v1.event.task_created"),
        ("task_completed", "hook.v1.event.task_completed"),
        ("teammate_idle", "hook.v1.event.teammate_idle"),
        (
            "on_compaction_started",
            "hook.v1.event.on_compaction_started",
        ),
        (
            "on_compaction_completed",
            "hook.v1.event.on_compaction_completed",
        ),
        ("config_changed", "hook.v1.event.config_changed"),
        ("instructions_loaded", "hook.v1.event.instructions_loaded"),
        ("file_changed", "hook.v1.event.file_changed"),
        ("cwd_changed", "hook.v1.event.cwd_changed"),
        ("worktree_created", "hook.v1.event.worktree_created"),
        ("worktree_removed", "hook.v1.event.worktree_removed"),
        (
            "elicitation_requested",
            "hook.v1.event.elicitation_requested",
        ),
        ("elicitation_resolved", "hook.v1.event.elicitation_resolved"),
        ("message_displayed", "hook.v1.event.message_displayed"),
        ("notification", "claude.v1.notification"),
    ];
    assert_eq!(HOOK_TOPIC_MAP, &expected);
}
