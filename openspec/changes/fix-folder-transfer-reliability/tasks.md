## 1. Transfer metadata and sender packaging

- [x] 1.1 Audit the current folder send path in `src/transfer.rs` and extract shared helpers for folder entry enumeration, path normalization, and archive manifest generation.
- [x] 1.2 Change folder packaging to fail fast on unreadable, unlistable, or unappendable entries instead of skipping them.
- [x] 1.3 Add folder manifest metadata to the transfer protocol, including normalized relative paths, entry types, file sizes, and per-file digests needed for receiver-side validation.
- [x] 1.4 Update sender logging and emitted transfer statuses so packaging failures are reported with explicit reasons.

## 2. Receiver extraction and validation

- [x] 2.1 Refactor folder receipt to write archives and extracted content into isolated staging paths under the receive directory.
- [x] 2.2 Validate archive entries during extraction, rejecting path traversal, absolute paths, and normalized path collisions.
- [x] 2.3 Implement post-extract manifest validation that checks required directories, file presence, sizes, and file digests before marking success.
- [x] 2.4 Promote the staged folder into the final destination only after validation succeeds, and ensure all temp artifacts are cleaned up on both success and failure paths.

## 3. Status reporting and compatibility behavior

- [x] 3.1 Update transfer progress events and UI messaging in `src/ui.rs` to distinguish packaging, receiving, extracting, validating, completed, and failed folder states.
- [x] 3.2 Define and implement backward-aware handling for peers that do not support the new folder-validation metadata, preferring explicit failure over silent downgrade.
- [x] 3.3 Ensure receive-map persistence and final reported local paths point only to validated final folder locations.

## 4. Verification

- [x] 4.1 Add automated tests for nested folder success, empty-directory preservation, unreadable-entry packaging failure, extraction failure, and manifest mismatch handling.
- [x] 4.2 Add targeted tests for unsafe archive paths and normalized path collisions on the receiving side.
- [x] 4.3 Update manual regression coverage in `doc/测试方法.md` for verified folder completion, interrupted transfer failure, and staged cleanup expectations.
