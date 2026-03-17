## Why

Current folder transfer can fail during send or receive, and in some cases the receiver may end up with a partially restored or internally disordered directory while the UI still reports a completed transfer. This is happening in a user-visible reliability path, so we need clearer transfer guarantees, stronger validation, and safer extraction behavior before more users depend on folder sending.

## What Changes

- Define a reliable folder transfer capability with explicit success criteria for packaging, transport, extraction, and final completion reporting.
- Require folder transfers to preserve directory structure exactly, including nested files and subdirectories.
- Require the receiver to validate the folder payload before marking the transfer complete, and to surface a failure when extraction is incomplete or inconsistent.
- Require better handling for transient transport failures so interrupted folder sends fail cleanly instead of producing silently corrupted output.
- Require observable diagnostics and test coverage for nested, large, and partially failing folder transfers.

## Capabilities

### New Capabilities
- `reliable-folder-transfer`: Guarantees correct packaging, transport, extraction, and completion reporting for folder sends, including nested directory preservation and failure visibility.

### Modified Capabilities

## Impact

- Affected code: `src/transfer.rs`, `src/net.rs`, `src/ui.rs`, and transfer-related tests.
- Affected behavior: folder send/receive progress reporting, completion state, unpack validation, and retry/failure handling.
- Dependencies/systems: TCP folder transfer path, tar packaging/extraction flow, local receive-map persistence, and manual regression tests for multi-level directories.
