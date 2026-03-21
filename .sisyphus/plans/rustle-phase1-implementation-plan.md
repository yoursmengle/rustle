# Rustle Phase 1 Implementation Plan

## Goal

Deliver the Phase 1 hardening slice with the best impact-to-risk ratio:

1. add `transfer_id`
2. propagate it through file transfer protocol and events
3. use it for completion/progress correlation instead of filename heuristics where possible
4. add explicit TCP transfer timeouts
5. bound transfer-related allocations before buffer creation

## Why this slice

Current evidence shows the highest correctness risk is same-name transfer ambiguity and indefinite transfer stalls.

Concrete hotspots already confirmed:

1. `src/transfer.rs` uses `receive_map_key(&sender_id, is_dir, &filename)`
2. `src/model.rs` `FileCompletionPayload` and `PeerEvent::{FileCompletionAck, FileProgress}` only expose filename-level identity
3. `src/net.rs` forwards file completion acknowledgments using filename only
4. `src/transfer.rs::handle_incoming_file` performs repeated `read_exact` calls without explicit timeout wrappers

## Scope

### In scope

1. `src/model.rs`
2. `src/transfer.rs`
3. `src/net.rs`
4. `src/ui.rs`
5. `src/history.rs` only if Phase 1 correlation needs additive transfer-aware helpers
6. `tests/transfer_header_tests.rs` and/or new targeted tests

### Out of scope

1. extracting delivery core from `ui.rs`
2. atomic persistence helper rollout
3. broad history storage redesign
4. metadata watcher redesign
5. network worker concurrency redesign

## Compatibility Strategy

Phase 1 must remain additive.

Rules:

1. old peers must still parse or safely ignore the protocol path they already understand
2. sender success should become more conservative, not more optimistic
3. if `transfer_id` is unavailable from an old peer, fallback behavior may remain filename-based, but only on the legacy path
4. the new path should prefer `transfer_id` everywhere it exists

Recommended compatibility mechanism:

1. keep current header layout prefix stable
2. append new optional transfer metadata in a backward-compatible extension segment, similar to existing folder-manifest style extension handling
3. keep `FileCompletionPayload` additive by adding an optional `transfer_id: Option<String>` field
4. keep `PeerEvent::FileCompletionAck` and `PeerEvent::FileProgress` additive by adding `transfer_id: Option<String>`

## Execution Order

### Step 1 - Expand protocol and event types

Files:

1. `src/model.rs`

Tasks:

1. add `transfer_id: Option<String>` to `FileCompletionPayload`
2. add `transfer_id: Option<String>` to `PeerEvent::FileCompletionAck`
3. add `transfer_id: Option<String>` to `PeerEvent::FileProgress`
4. add or reuse a transfer-id generator strategy for outbound transfers
5. update serde roundtrip tests for additive payload compatibility

Expected result:

1. transfer-related events can carry stable correlation identity without breaking old payload parsing

Verification:

1. `cargo test chat_ack_discover_serde`
2. add and run `cargo test file_completion_payload_accepts_optional_transfer_id`

### Step 2 - Thread transfer_id through send path

Files:

1. `src/transfer.rs`
2. any transfer-start callsites in `src/ui.rs` or `src/net.rs`

Tasks:

1. generate `transfer_id` when a send starts
2. include `transfer_id` in outbound progress events
3. include `transfer_id` in outbound completion-ack emission
4. keep filename in events for UI display, but stop using it as the primary identity on the new path

Expected result:

1. every new outbound transfer has a stable identity from start to completion

Verification:

1. add and run `cargo test outgoing_transfer_emits_consistent_transfer_id`

### Step 3 - Parse transfer_id on incoming path

Files:

1. `src/transfer.rs`

Tasks:

1. extend incoming header parsing to read optional transfer-id extension data
2. preserve legacy parsing path when extension is missing
3. bound transfer-id length and any optional extension lengths before allocation
4. plumb parsed `transfer_id` through progress and completion events

Expected result:

1. new peers use transfer-aware correlation, old peers still work

Verification:

1. add and run `cargo test legacy_peer_without_transfer_id_or_completion_ack_remains_pending`
2. add and run `cargo test header_roundtrip_with_optional_transfer_id_extension`

### Step 4 - Replace filename-first correlation with transfer-aware correlation

Files:

1. `src/ui.rs`
2. `src/transfer.rs`
3. `src/history.rs` if needed

Tasks:

1. update any transfer completion/progress match logic to prefer `transfer_id`
2. update receive-map key derivation to prefer `transfer_id` when present
3. keep legacy filename-based fallback only when no transfer identity exists
4. isolate fallback branches so they are easy to remove later

Expected result:

1. same-named transfers no longer overwrite each other on the new path

Verification:

1. add and run `cargo test same_name_transfers_keep_distinct_status`

### Step 5 - Add explicit timeout wrappers

Files:

1. `src/transfer.rs`

Tasks:

1. define constants for connect timeout, header read timeout, body read timeout, and final promotion timeout
2. wrap `TcpStream::connect` and repeated `read_exact` phases with `tokio::time::timeout`
3. map timeout failure into explicit transfer failure/progress messages
4. ensure timeout exits trigger the same cleanup path as other failures

Expected result:

1. half-open or stalled transfers terminate predictably

Verification:

1. add and run `cargo test transfer_connect_timeout_marks_failure`
2. add and run `cargo test transfer_body_read_timeout_marks_failure`

### Step 6 - Bound transfer allocations

Files:

1. `src/transfer.rs`

Tasks:

1. add maximum allowed lengths for optional manifest and transfer-id extension fields
2. reject invalid lengths before allocation
3. return explicit failure status when bounds are exceeded

Expected result:

1. malformed payloads fail fast without unbounded allocation

Verification:

1. add and run `cargo test oversized_manifest_is_rejected_before_allocation`
2. add and run `cargo test oversized_transfer_id_extension_is_rejected`

## File-by-File Change Intent

### `src/model.rs`

Make transfer-related payloads/events correlation-aware but additive.

### `src/transfer.rs`

This is the main Phase 1 file.

Planned responsibilities in this phase:

1. generate transfer ids
2. parse optional transfer id from inbound header
3. emit richer progress/completion events
4. apply timeouts to connect/read phases
5. reject oversized extension fields before allocation

### `src/net.rs`

Only adapt parsing/forwarding shape as needed.

Do not redesign transport ownership here.

### `src/ui.rs`

Only update event correlation logic.

Do not start the larger extraction yet.

### `src/history.rs`

Touch only if required to avoid filename-only updates in the new path.

If a history change is required, it must be additive and minimal.

## Testing Plan

### New named tests expected from Phase 1

1. `file_completion_payload_accepts_optional_transfer_id`
2. `outgoing_transfer_emits_consistent_transfer_id`
3. `header_roundtrip_with_optional_transfer_id_extension`
4. `same_name_transfers_keep_distinct_status`
5. `legacy_peer_without_transfer_id_or_completion_ack_remains_pending`
6. `transfer_connect_timeout_marks_failure`
7. `transfer_body_read_timeout_marks_failure`
8. `oversized_manifest_is_rejected_before_allocation`
9. `oversized_transfer_id_extension_is_rejected`

### Existing regression gate

1. `cargo test`

## Risk Controls

### Risk 1 - breaking old peers

Mitigation:

1. keep `transfer_id` optional in payloads/events
2. preserve legacy parse path when extension field is absent
3. keep sender UI in waiting/unknown state rather than claiming confirmed success on legacy path

### Risk 2 - touching too much UI logic

Mitigation:

1. only patch correlation points in `ui.rs`
2. defer state ownership cleanup to Phase 3

### Risk 3 - timeout changes break normal transfers

Mitigation:

1. use conservative timeout values first
2. limit timeout scope to clearly blocking phases
3. verify existing transfer tests still pass

## Done Criteria

Phase 1 is done only when:

1. new transfers carry `transfer_id`
2. completion/progress correlation prefers `transfer_id`
3. same-name transfer test passes
4. explicit timeout tests pass
5. oversize-bound tests pass
6. full `cargo test` passes

## Recommended First Implementation Batch

Keep the first actual coding batch even smaller than the whole phase:

1. Step 1
2. Step 2
3. Step 3
4. the minimal test subset needed to prove transfer-id propagation works

Then stop and verify before touching timeout logic.
