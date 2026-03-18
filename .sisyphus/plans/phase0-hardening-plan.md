# Phase 0 Hardening Plan

## Goal

Bring Rustle to a usable reliability baseline without a broad refactor.

## Scope

1. Persist stable `node_id`
2. Persist and restore runtime reliability state for chat:
   - `offline_msgs`
   - `pending_acks`
3. Harden regular file receive using temp/staging writes plus cleanup on failure
4. Add receiver-confirmed file completion acknowledgments so sender state reflects peer-confirmed completion, not just local TCP write completion
5. Add tests first for the new helpers and protocol types, then run full test verification

## Out of Scope

1. Full `ui.rs` decomposition
2. Concurrent outbound file transfer redesign
3. Persistence for `offline_sync`
4. Sync architecture unification
5. Cross-platform support work beyond keeping behavior compatible

## Constraints

1. Preserve existing chat ACK behavior
2. Preserve existing reliable-folder validation behavior
3. Keep peer compatibility as graceful as possible
4. Do not break existing history loading and known-peer loading
5. Prefer additive protocol changes over breaking changes

## Execution Order

### Step 1 - Slice A: stable identity persistence (TDD)

1. Add failing tests for persisted `node_id` helper behavior
2. Implement persisted `node_id` loading/saving
3. Refactor only after tests are green

### Step 2 - Slice B: runtime reliability state persistence (TDD)

1. Add failing tests for runtime queue-state serialize/deserialize roundtrip
2. Add failing tests for startup restoration of `offline_msgs` / `pending_acks`
3. Implement persistence and restoration helpers
4. Refactor only after tests are green

### Step 3 - Slice C: regular-file staged receive hardening (TDD)

1. Add failing tests for staged regular-file success/failure cleanup helpers
2. Implement temp/staging file write, validate, promote, and cleanup logic
3. Refactor only after tests are green

### Step 4 - Slice D: receiver-confirmed file completion ACK (TDD)

1. Add failing tests for file completion ACK payload serde and sender-side status handling
2. Implement additive protocol/event path
3. Refactor only after tests are green

### Step 5 - Stable identity persistence

In `storage.rs`:

1. add a dedicated persisted node-id path under `data_dir()`
2. load existing id if present and valid
3. otherwise derive from machine UUID when available or generate UUID
4. save the chosen id for future runs

### Step 6 - Runtime reliability state persistence

Add a dedicated JSON state file under `data_dir()`.

Persist:

1. per-peer offline queued chat/file items
2. per-peer pending chat ACK items with enough data to retry after restart

Rules:

1. persist after material mutations to `offline_msgs` or `pending_acks`
2. restore after known peers/history load during startup
3. restoration must repopulate in-memory retry structures without duplicating visible messages
4. restoration must mark pending outgoing chat messages as pending/waiting if needed

### Step 7 - Regular file receive hardening

In `transfer.rs` regular-file receive path:

1. write incoming content to a temp/staging file inside transfer root
2. validate size/hash against the temp file
3. atomically rename into final destination only on success
4. delete temp file on all failure paths
5. preserve existing local-path and progress reporting semantics as much as possible

### Step 8 - Receiver-confirmed file completion ACK

Add a new UDP payload/event path for file completion acknowledgment.

Requirements:

1. receiver sends success/failure final status after final validation/commit
2. sender UI updates final transfer status from receiver confirmation
3. sender should not claim final success solely from local socket write completion
4. protocol must be additive so old peers do not break
5. if no completion ACK is available, sender should remain in a waiting/unknown state rather than falsely claim success

### Step 9 - Full verification

Run targeted tests first, then `cargo test`.

## Compatibility Risks

1. Old peers will not understand the new file completion ACK payload
2. Restoring pending ACK state incorrectly may create duplicate resend attempts
3. Restoring offline queue state incorrectly may duplicate pending UI state already loaded from history

## Mitigations

1. Make file completion ACK optional/additive
2. Restore runtime state by message ID/path identity and dedupe against existing in-memory state
3. Keep chat ACK path unchanged
4. Preserve sender-visible “waiting for peer confirmation” state for file sends when peer support is unknown

## Executable QA Scenarios

### QA for Slice A - persisted node_id

- Tool: `cargo test`
- Steps:
  1. run targeted storage tests for node-id persistence helpers
  2. call the helper twice against the same persisted path
- Expected Results:
  1. the first call returns a non-empty id and writes it
  2. the second call returns the exact same id

### QA for Slice B - runtime reliability state persistence/recovery

- Tool: `cargo test`
- Steps:
  1. serialize queued offline chat/file items and pending ACK state
  2. deserialize and restore into runtime structures
  3. simulate startup restoration on top of existing visible message history
- Expected Results:
  1. no queued item is lost
  2. pending ACK retry metadata is restored in usable form
  3. no duplicate visible messages are created during restoration

### QA for Slice C - staged regular-file receive

- Tool: `cargo test`
- Steps:
  1. write an incoming regular file to a temp/staging path
  2. simulate validation success and promote it
  3. simulate validation failure and cleanup
- Expected Results:
  1. success path leaves the final file in place and no temp file remains
  2. failure path leaves no partial final file and no temp file remains

### QA for Slice D - additive file completion ACK compatibility

- Tool: `cargo test`
- Steps:
  1. serialize/deserialize the new completion ACK payload
  2. simulate sender status flow with receiver-confirmed success/failure
  3. simulate missing completion ACK from an old peer
- Expected Results:
  1. payload roundtrip succeeds
  2. sender final status changes only after receiver confirmation
  3. when completion ACK is missing, sender remains waiting/unknown rather than falsely successful
  4. chat ACK behavior remains unchanged

### Final QA

- Tool: `cargo test`
- Steps:
  1. run the full test suite after all slices are green
- Expected Results:
  1. all tests pass

## Rollback Strategy

1. Keep new persistence isolated to dedicated helper functions and files
2. Keep protocol addition in separate payload/event handling branches
3. If receiver-confirmed file ACK proves too risky, retain staged receive hardening and disable only the sender confirmation path rather than reverting identity/state persistence

## Atomic Commit Strategy

1. Commit 1: Slice A green - persisted `node_id` tests + implementation
2. Commit 2: Slice B green - runtime reliability state tests + implementation
3. Commit 3: Slice C green - staged regular-file receive tests + implementation
4. Commit 4: Slice D green - additive file completion ACK tests + implementation
5. Final verification before any PR/merge work
