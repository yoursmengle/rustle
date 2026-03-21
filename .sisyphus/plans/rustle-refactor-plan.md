# Rustle Refactor Plan

## Goal

Reduce correctness risk and maintenance cost in Rustle without a rewrite.

Success means:

1. reliability state no longer depends on `ui.rs` frame-loop behavior
2. file transfers are correlated by stable transfer identity rather than filename
3. stalled or half-open transfers fail predictably via explicit timeouts
4. critical persisted state is written atomically and surfaced on failure
5. behavior is covered by scenario tests for restart recovery, duplicates, and same-name transfers

## Non-Goals

1. Replacing egui
2. Replacing Tokio or the full networking model in one pass
3. Building authentication or Internet-facing security features
4. Reformatting the whole codebase
5. Large protocol redesign beyond additive compatibility changes

## Current Pain Points

1. `src/ui.rs` owns delivery reliability, retry queues, dedup, and persistence triggers
2. `src/transfer.rs` and `src/ui.rs` correlate transfers by filename/path heuristics
3. TCP transfer phases lack explicit connect/read/write/commit timeouts
4. `src/history.rs` and `src/storage.rs` rely on synchronous, mostly non-atomic file rewrites with many silent failures
5. `src/metadata.rs` watcher and UI both mutate sync-related state with weak ownership boundaries
6. duplicate-message suppression is in-memory and reset-prone

## Target Architecture

### 1. UI Layer

`src/ui.rs` should become a thin view/controller layer that:

1. renders state
2. forwards user intents
3. consumes domain events
4. does not own retry/dedup/delivery truth

### 2. Delivery Core Layer

Add a new module group, for example:

- `src/delivery/mod.rs`
- `src/delivery/state.rs`
- `src/delivery/reducer.rs`
- `src/delivery/types.rs`

This layer should own:

1. pending chat ACK state
2. offline outbound queue state
3. recent inbound message/transfer dedup window
4. transfer session state transitions
5. restart restoration rules

### 3. Transfer Protocol Layer

Keep file IO in `src/transfer.rs`, but move protocol identity to explicit transfer metadata:

1. `transfer_id`
2. transfer kind (`file` / `dir`)
3. sender id
4. display filename
5. optional capability/version flags

### 4. Persistence Layer

Keep `storage.rs` as persistence utility surface, but separate APIs by concern:

1. `settings`
2. `runtime_state`
3. `receive_map`
4. `history`
5. `sync_tree`

All critical writes should be temp-file + rename based where practical.

## Phase Plan

---

## Phase 1 - Protocol Hardening First

### Objective

Fix the highest-risk correctness bugs without moving major architecture yet.

### Changes

#### A. Introduce `transfer_id`

Files:

1. `src/model.rs`
2. `src/transfer.rs`
3. `src/ui.rs`
4. `src/history.rs`
5. `src/storage.rs`

Actions:

1. extend file-transfer header/payload flow with a stable `transfer_id`
2. include `transfer_id` in progress and completion events
3. use `transfer_id` for receive-map keys and UI correlation
4. stop matching completion state by filename suffix heuristics where transfer-aware matching is possible

Expected result:

1. same peer can send two same-named files/directories without wrong status updates

#### B. Add explicit TCP timeouts

Files:

1. `src/transfer.rs`
2. `src/storage.rs` or new config constants location

Actions:

1. add timeout constants/settings for connect, header read, body read, and final commit wait
2. wrap `TcpStream::connect`, `read_exact`, streaming reads, and blocking promotion steps where appropriate
3. convert timeout failures into explicit user-visible progress/error states

Expected result:

1. half-open transfers do not stall forever

#### C. Bound protocol allocations

Files:

1. `src/transfer.rs`

Actions:

1. cap manifest length before allocating buffers
2. reject obviously invalid header sizes early
3. preserve additive compatibility for old peers when possible

Expected result:

1. malformed or hostile payloads fail fast instead of consuming uncontrolled memory

### Phase 1 Verification

#### QA 1A - same-name transfer correlation

- Tool:
  1. `cargo test same_name_transfers_keep_distinct_status`
- Setup:
  1. add an integration or module test that creates two transfer sessions from the same peer with the same display filename and different `transfer_id`
  2. feed completion/progress events through the correlation path used by UI or delivery state
- Steps:
  1. run `cargo test same_name_transfers_keep_distinct_status`
- Expected result:
  1. each transfer updates only its own status/history entry
  2. no receive-map key or completion event overwrites the other transfer

#### QA 1B - legacy peer compatibility without transfer confirmation

- Tool:
  1. `cargo test legacy_peer_without_transfer_id_or_completion_ack_remains_pending`
- Setup:
  1. add a protocol compatibility test that simulates an old peer path missing the new transfer correlation fields or completion confirmation
- Steps:
  1. run `cargo test legacy_peer_without_transfer_id_or_completion_ack_remains_pending`
- Expected result:
  1. the sender does not mark the transfer as confirmed success
  2. the sender remains in waiting/unknown state or compatible fallback state
  3. the old peer path does not crash header parsing

#### QA 1C - stalled transfer timeout behavior

- Tool:
  1. `cargo test transfer_connect_timeout_marks_failure`
  2. `cargo test transfer_body_read_timeout_marks_failure`
- Setup:
  1. add tests using a local listener or simulated socket behavior that accepts but does not complete the expected transfer phase
- Steps:
  1. run `cargo test transfer_connect_timeout_marks_failure`
  2. run `cargo test transfer_body_read_timeout_marks_failure`
- Expected result:
  1. timeout is surfaced as explicit failure state
  2. no task remains blocked indefinitely
  3. temp/staging artifacts are cleaned up on timeout failure

#### QA 1D - full regression gate

- Tool:
  1. `cargo test`
- Steps:
  1. run `cargo test`
- Expected result:
  1. all existing and new tests pass

### Why Phase 1 comes first

It closes correctness gaps immediately without requiring large structural change.

---

## Phase 2 - Persistence Hardening

### Objective

Make state durable and predictable under crash/restart conditions.

### Changes

#### A. Add atomic write helpers

Files:

1. `src/storage.rs`
2. `src/history.rs`

Actions:

1. add shared helper for `write_json_atomic` / `write_text_atomic`
2. update `save_settings`, `save_runtime_state`, `save_receive_map`, `save_sync_tree`
3. update history rewrite path to use temp file + replace instead of direct overwrite

Expected result:

1. partial file corruption risk is reduced on crash/interruption

#### B. Stop silent failure in critical paths

Files:

1. `src/storage.rs`
2. `src/history.rs`
3. `src/ui.rs`
4. `src/transfer.rs`

Actions:

1. convert critical `let _ = ...` persistence writes into surfaced `Result` or logged failures
2. keep best-effort cleanup as best-effort, but separate it from state durability writes

Expected result:

1. state write failures become diagnosable

#### C. Protect concurrent read-modify-write state

Files:

1. `src/storage.rs`
2. `src/transfer.rs`
3. `src/ui.rs`

Actions:

1. isolate `receive_map` updates behind helper APIs rather than ad hoc load/modify/save sequences
2. ensure concurrent transfer completion cannot trivially clobber prior map entries

Expected result:

1. receive-path persistence is no longer race-prone by design

### Phase 2 Verification

#### QA 2A - runtime state roundtrip

- Tool:
  1. `cargo test runtime_state_roundtrip_preserves_offline_and_pending`
  2. `cargo test restore_runtime_state_deduplicates_visible_messages`
- Setup:
  1. add a restoration-focused test that loads persisted runtime state on top of already loaded visible history
- Steps:
  1. run `cargo test runtime_state_roundtrip_preserves_offline_and_pending`
  2. run `cargo test restore_runtime_state_deduplicates_visible_messages`
- Expected result:
  1. offline queue and pending ack data restore correctly
  2. visible messages are not duplicated during startup restoration

#### QA 2B - receive-map concurrent update protection

- Tool:
  1. `cargo test receive_map_updates_do_not_clobber_existing_entries`
- Setup:
  1. add a test that performs sequentially interleaved helper-level updates representing concurrent transfer completions
- Steps:
  1. run `cargo test receive_map_updates_do_not_clobber_existing_entries`
- Expected result:
  1. all inserted entries remain present after multiple updates
  2. no earlier entry is lost when a later write occurs

#### QA 2C - atomic history rewrite

- Tool:
  1. `cargo test history_rewrite_preserves_unrelated_lines`
  2. `cargo test atomic_write_helper_replaces_complete_file`
- Steps:
  1. run `cargo test history_rewrite_preserves_unrelated_lines`
  2. run `cargo test atomic_write_helper_replaces_complete_file`
- Expected result:
  1. targeted history fields change without removing unrelated entries
  2. atomic write helper leaves a complete final file, not a partial one

#### QA 2D - full regression gate

- Tool:
  1. `cargo test`
- Steps:
  1. run `cargo test`
- Expected result:
  1. all existing and new tests pass

---

## Phase 3 - Extract Delivery State from UI

### Objective

Remove reliability logic from `RustleApp` so delivery semantics are testable without egui.

### Changes

#### A. Define a delivery state model

New files:

1. `src/delivery/types.rs`
2. `src/delivery/state.rs`

Suggested responsibilities:

1. pending outbound chat messages
2. pending transfer sessions
3. offline queue
4. recent inbound IDs
5. peer liveness-derived resend rules

#### B. Introduce event-driven state transitions

New files:

1. `src/delivery/reducer.rs`

Actions:

1. define input events such as `ChatQueued`, `ChatSent`, `AckReceived`, `PeerOnline`, `TransferProgress`, `TransferCompleted`, `RestartRestored`
2. return deterministic state mutations and follow-up intents

Expected result:

1. message/transfer reliability behavior can be tested without UI frame execution

#### C. Reduce `ui.rs` responsibilities

Files:

1. `src/ui.rs`

Actions:

1. keep rendering, local widget state, and command dispatch in UI
2. move pending-ack queue mutation, dedup mutation, and resend scheduling decisions into delivery core
3. UI consumes derived display state rather than constructing truth ad hoc

Expected result:

1. `ui.rs` shrinks materially and becomes easier to change safely

### Phase 3 Verification

#### QA 3A - delivery reducer behavior

- Tool:
  1. `cargo test delivery_reducer_ack_timeout_schedules_retry`
  2. `cargo test delivery_reducer_peer_online_flushes_queue`
- Steps:
  1. run `cargo test delivery_reducer_ack_timeout_schedules_retry`
  2. run `cargo test delivery_reducer_peer_online_flushes_queue`
- Expected result:
  1. reducer emits deterministic retry/flush decisions from state transitions
  2. no egui context is required to test the behavior

#### QA 3B - restart restoration through delivery core

- Tool:
  1. `cargo test delivery_restore_rebuilds_pending_state_without_duplication`
- Steps:
  1. run `cargo test delivery_restore_rebuilds_pending_state_without_duplication`
- Expected result:
  1. pending messages and transfer sessions are reconstructed correctly
  2. restoration does not duplicate already loaded visible messages

#### QA 3C - duplicate inbound message suppression

- Tool:
  1. `cargo test delivery_dedup_suppresses_duplicate_chat_event`
- Steps:
  1. run `cargo test delivery_dedup_suppresses_duplicate_chat_event`
- Expected result:
  1. repeated inbound event with same identity produces one visible message only

#### QA 3D - UI smoke verification

- Tool:
  1. `cargo run`
- Setup:
  1. launch one local instance
  2. verify app startup with existing persisted data still succeeds after the delivery-core extraction
- Steps:
  1. run `cargo run`
  2. open the app
  3. confirm main window renders, peer list area renders, history loads, and sending a local draft message still updates UI state without panic
- Expected result:
  1. application starts successfully
  2. primary chat UI remains functional
  3. no startup panic or immediate state-restoration regression appears

#### QA 3E - full regression gate

- Tool:
  1. `cargo test`
- Steps:
  1. run `cargo test`
- Expected result:
  1. all existing and new tests pass

### Scope guard

Do not rewrite network socket code yet. Only change how state is owned.

---

## Phase 4 - Dedup and Recovery Semantics

### Objective

Make reliability semantics survive restarts and high-volume usage.

### Changes

#### A. Persist bounded dedup window

Files:

1. `src/delivery/state.rs`
2. `src/storage.rs`
3. `src/ui.rs` or delivery integration point

Actions:

1. store a bounded recent inbound `msg_id` / `transfer_id` window
2. replace clear-all dedup behavior with bounded eviction

Expected result:

1. duplicate UDP delivery after restart or long sessions is handled more predictably

#### B. Refine ACK timeout semantics

Files:

1. delivery core
2. `src/net.rs`
3. `src/ui.rs`

Actions:

1. separate `delayed` from `offline`
2. avoid flipping to hard offline state purely on a single aggressive timeout
3. preserve current user-visible behavior where possible but reduce false negatives

Expected result:

1. lossy LAN conditions create degraded state, not incorrect state

### Phase 4 Verification

#### QA 4A - delayed ACK behavior

- Tool:
  1. `cargo test delayed_ack_keeps_message_in_degraded_not_offline_state`
- Steps:
  1. run `cargo test delayed_ack_keeps_message_in_degraded_not_offline_state`
- Expected result:
  1. a delayed ack does not immediately force the peer/message into hard offline failure semantics
  2. state remains retryable and diagnosable

#### QA 4B - duplicate arrival after restart

- Tool:
  1. `cargo test persisted_dedup_window_blocks_duplicate_after_restart`
- Steps:
  1. run `cargo test persisted_dedup_window_blocks_duplicate_after_restart`
- Expected result:
  1. duplicate inbound message/transfer event after restoration is ignored when it is still inside the dedup window

#### QA 4C - resend when peer returns online

- Tool:
  1. `cargo test peer_online_event_releases_retryable_queue`
- Steps:
  1. run `cargo test peer_online_event_releases_retryable_queue`
- Expected result:
  1. retryable outbound items are released when peer returns online
  2. already-confirmed items are not resent

#### QA 4D - full regression gate

- Tool:
  1. `cargo test`
- Steps:
  1. run `cargo test`
- Expected result:
  1. all existing and new tests pass

---

## Phase 5 - Network Worker Boundary Cleanup

### Objective

Simplify long-term maintenance without destabilizing the app.

### Changes

Files:

1. `src/net.rs`
2. `src/model.rs`
3. delivery integration files

Actions:

1. document and narrow `NetCmd` / `PeerEvent` responsibilities
2. isolate UDP parsing from peer-state mutation where possible
3. reduce direct state semantics in `net.rs` to transport/event emission
4. consider replacing mixed std mpsc + manual polling with a more consistent async/event approach only after earlier phases are stable

Expected result:

1. transport layer becomes easier to reason about and less coupled to app state

### Scope guard

This phase is optional until Phases 1-4 are complete and stable.

---

## File-by-File Refactor Map

### `src/ui.rs`

Refactor target:

1. keep rendering and interaction only
2. remove ownership of pending ACK truth
3. remove filename/path-based transfer completion matching
4. reduce direct persistence orchestration where possible

### `src/transfer.rs`

Refactor target:

1. own file IO and transfer protocol framing
2. gain `transfer_id`, timeouts, and bounded allocations
3. emit richer typed events; avoid UI-specific assumptions

### `src/net.rs`

Refactor target:

1. stay transport-focused
2. emit parsed events rather than owning cross-cutting reliability state

### `src/history.rs`

Refactor target:

1. move away from ambiguous filename-based updates where possible
2. use atomic rewrite helpers

### `src/storage.rs`

Refactor target:

1. centralize durable write helpers
2. expose focused APIs for runtime state and receive-map updates

### `src/metadata.rs`

Refactor target:

1. keep watcher behavior isolated
2. integrate with delivery/sync state through clearer boundaries later

## Recommended Sequencing

1. Phase 1 first
2. Phase 2 second
3. Phase 3 third
4. Phase 4 fourth
5. Phase 5 last and only if still valuable

This order preserves working behavior while improving correctness incrementally.

## Risks and Mitigations

### Risk 1: protocol compatibility break

Mitigation:

1. keep transfer-id addition additive when possible
2. bump or gate by capability/version field
3. keep fallback path for old peers

### Risk 2: refactor accidentally changes user-visible delivery semantics

Mitigation:

1. codify current behavior in tests before extraction
2. only then move logic into delivery core

### Risk 3: over-refactor `ui.rs`

Mitigation:

1. move state ownership first
2. postpone cosmetic file splits until behavior stabilizes

### Risk 4: Windows file replacement edge cases

Mitigation:

1. validate atomic write helper behavior on Windows paths
2. preserve existing long-path helpers

## Test Strategy

### Must-add scenario tests

1. same peer sends `file.txt` twice and both completions map correctly
2. duplicate UDP chat packet only materializes once
3. restart with pending ACK state restores cleanly without duplicate visible messages
4. stalled TCP header/body read times out with explicit failure state
5. concurrent receive-map updates do not lose entries

For each scenario above, implementation is not complete until there is either:

1. a named `cargo test <name>` case checked into the repo, or
2. a documented manual repro using `cargo run` with explicit steps and expected result

### Existing verification gate

1. `cargo test` must stay green after each phase

## Delivery Format for Implementation Work

When implementing this plan later, use small slices:

1. one protocol change slice
2. one persistence hardening slice
3. one delivery-core extraction slice

Do not combine Phase 1 and Phase 3 in a single implementation batch.

## Recommended First Implementation Slice

If implementation starts next, do this exact slice first:

1. add `transfer_id`
2. propagate it through transfer events and completion ACKs
3. update receive-map/history correlation to prefer `transfer_id`
4. add tests for same-name transfer correctness

This slice has the best impact-to-risk ratio in the repository today.
