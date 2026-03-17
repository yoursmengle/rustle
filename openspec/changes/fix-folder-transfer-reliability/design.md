## Context

Rustle currently sends folders by first building a temporary `.tar` archive in `src/transfer.rs`, then streaming that archive over TCP and unpacking it directly into the receiver's destination directory. The current flow has several reliability gaps:

- Folder packaging can silently skip unreadable entries and still continue, which means the sender can report success for an incomplete archive.
- The receiver unpacks directly into the final destination, so an extraction failure can leave a partially restored folder mixed with missing files.
- Folder transfers do not get end-to-end content validation after receipt; file SHA256 is validated, but directory transfers explicitly skip that verification.
- Completion is tied mostly to byte-count success and socket completion, not to verified folder integrity.

This is a cross-cutting change spanning sender packaging, transport metadata, receiver extraction, UI status reporting, and tests.

## Goals / Non-Goals

**Goals:**
- Ensure folder transfer only reports success after packaging, transport, and extraction all complete successfully.
- Preserve nested directory structure exactly as sent.
- Prevent failed extraction from polluting the final receive directory with partial output.
- Make likely failure causes observable in logs and visible in transfer status.
- Add automated and manual coverage for nested folder and failure-path transfers.

**Non-Goals:**
- Replacing TAR with a different archive format.
- Implementing resumable folder transfer.
- Redesigning the entire network transport protocol beyond metadata needed for folder reliability.

## Decisions

### Stage received folders before final placement
The receiver will write the archive to a temporary file and extract into a dedicated temporary staging directory instead of the final destination. Only after extraction and validation succeed will the staged root be moved into the final location.

Rationale: this prevents partially unpacked folders from appearing as completed results and gives the receiver a clean rollback boundary.

Alternatives considered:
- Extract directly into the final directory and clean up on failure: rejected because cleanup after partial extraction is error-prone and can delete preexisting content accidentally.
- Extract each entry to an in-memory manifest first: rejected as too memory-heavy for large folders.

### Treat skipped or unreadable source entries as transfer failure
Folder packaging will no longer tolerate skipped entries as a soft warning. If a source entry cannot be enumerated, read, or appended to the archive, the sender SHALL fail the transfer and report the reason.

Rationale: a "successful" archive with omitted files is indistinguishable from corruption from the user's perspective.

Alternatives considered:
- Keep best-effort packaging and show a warning: rejected because users still receive incomplete folders and may not notice the warning.

### Add explicit folder payload validation metadata
Folder transfers will include verifiable metadata beyond raw archive size. The preferred design is a manifest captured at packaging time containing normalized relative paths, entry type, per-file size, and per-file digest, plus aggregate counts. The receiver validates extracted output against that manifest before marking success.

Rationale: byte-complete TAR delivery does not prove that unpacked content matches the sender's intended tree.

Alternatives considered:
- Validate only the TAR file checksum: rejected because it proves transport integrity but not extraction completeness or final on-disk structure.
- Validate only entry counts: rejected because it misses file-content corruption and path mismatches.

### Make completion status depend on verified extraction
Folder transfer status in the UI and emitted events will remain in a receiving or validating state until all validation passes. The final success state will be emitted only after extraction and manifest checks succeed.

Rationale: current progress behavior can imply success too early.

Alternatives considered:
- Keep current "received bytes == success" behavior and add post-processing logs: rejected because UI would still mislead users.

### Normalize path handling and reject unsafe archive entries
All folder entries will be normalized to relative paths rooted under the selected folder name. The receiver will reject entries that escape the target root, collide after normalization, or violate platform path constraints.

Rationale: this protects against path confusion and reduces cases where extracted results appear disordered due to conflicting or unsafe paths.

Alternatives considered:
- Trust TAR entry names as produced: rejected because malformed or duplicate paths can create inconsistent output on extraction.

## Risks / Trade-offs

- [Larger metadata and validation cost] -> Manifest generation and post-extract verification add CPU and I/O overhead; mitigate by streaming metadata generation during packaging and limiting hashing work to files only.
- [Temporary disk usage increases] -> Staging requires both the archive and extracted folder during receipt; mitigate by documenting space expectations and cleaning temp artifacts aggressively on every exit path.
- [Protocol compatibility complexity] -> New folder metadata may require version-tolerant parsing; mitigate by preserving a clear fallback path and rejecting unsupported directory-validation modes with a visible error instead of silent downgrade.
- [Move/rename edge cases on Windows] -> Atomic promotion may fail across volumes or when files are locked; mitigate by staging under the same base receive directory and falling back to a guarded copy-then-swap only when necessary.

## Migration Plan

- Implement sender-side strict packaging and receiver-side staging behind the existing folder transfer path.
- Extend folder transfer metadata in a backward-aware manner, with explicit logging when the peer does not support required validation.
- Add regression tests for nested folders, unreadable-entry failure, extraction failure, and success-state reporting.
- Roll out without user migration steps because this changes transfer behavior, not persisted user configuration.
- If rollback is needed, revert to the previous transfer path while preserving any newly received staged folders as ordinary failed transfers to avoid data loss.

## Open Questions

- Whether manifest data should be sent inline in the header or as a prelude block before archive bytes.
- Whether empty directories need explicit manifest entries or can be derived from file paths plus TAR structure.
- How much backward compatibility is required between old and new versions for folder sending, versus hard-failing incompatible peers.
