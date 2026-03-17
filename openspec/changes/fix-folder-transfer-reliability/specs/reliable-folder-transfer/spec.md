## ADDED Requirements

### Requirement: Folder transfer preserves the full directory tree
The system SHALL package and restore a sent folder such that the receiver obtains the same relative directory structure, including nested subdirectories, files, and empty directories under the selected root folder name.

#### Scenario: Nested folder transfer succeeds
- **WHEN** a user sends a folder containing multiple levels of subdirectories and files
- **THEN** the receiver SHALL restore the same relative tree under a single root folder with all expected entries present

#### Scenario: Empty subdirectory is preserved
- **WHEN** a user sends a folder that contains an empty subdirectory
- **THEN** the receiver SHALL create that empty subdirectory in the restored folder

### Requirement: Folder packaging fails on missing or unreadable entries
The system SHALL fail a folder transfer before network transmission completes if any selected source entry cannot be enumerated, read, or appended to the archive, and it SHALL surface the transfer as failed instead of silently omitting the entry.

#### Scenario: Unreadable child file aborts packaging
- **WHEN** the sender encounters a file inside the selected folder that cannot be read during archive creation
- **THEN** the folder transfer SHALL be marked failed and SHALL NOT be reported as successfully sent

#### Scenario: Directory enumeration fails
- **WHEN** the sender cannot enumerate part of the selected folder tree during packaging
- **THEN** the folder transfer SHALL stop and report a packaging failure reason

### Requirement: Folder receive completion requires verified extraction
The system SHALL mark a folder transfer as complete only after the full payload is received, extraction succeeds, and the extracted result is validated against transfer metadata.

#### Scenario: Transport completes but extraction fails
- **WHEN** the receiver has downloaded all declared folder bytes but archive extraction fails
- **THEN** the transfer SHALL be marked failed and SHALL NOT emit a completed folder state

#### Scenario: Extraction succeeds but validation detects mismatch
- **WHEN** the receiver extracts the folder payload and validation finds a missing, unexpected, or content-mismatched entry
- **THEN** the transfer SHALL be marked failed and SHALL surface that validation failure

### Requirement: Failed folder receive does not publish partial output
The system SHALL extract received folders into isolated temporary staging and SHALL only publish the restored folder into the final receive location after validation succeeds.

#### Scenario: Failure during extraction leaves no final folder
- **WHEN** a folder transfer fails during extraction or validation
- **THEN** the final receive location SHALL NOT contain a partially restored copy of the target folder

#### Scenario: Successful validation promotes staged folder
- **WHEN** extraction and validation both succeed
- **THEN** the receiver SHALL promote the staged folder into the final receive location and report that location to the UI

### Requirement: Folder transfer status reflects validation lifecycle
The system SHALL expose folder transfer states that distinguish packaging, sending, receiving, extracting, validating, success, and failure so that users are not shown a successful transfer before verification completes.

#### Scenario: Receiver is validating after bytes arrive
- **WHEN** all folder bytes have been received and extraction or validation is still running
- **THEN** the UI SHALL show an in-progress status rather than a completed status

#### Scenario: Validation finishes successfully
- **WHEN** the receiver finishes extraction and validation without mismatch
- **THEN** the UI SHALL update the folder transfer to a completed status

### Requirement: Folder transfer rejects unsafe or conflicting paths
The system SHALL reject folder archive entries that are absolute, escape the selected root through path traversal, or collide after normalization on the receiving platform.

#### Scenario: Archive entry escapes destination root
- **WHEN** the receiver encounters an entry path that resolves outside the target folder root
- **THEN** the transfer SHALL be marked failed and the entry SHALL NOT be written to disk

#### Scenario: Two entries normalize to the same destination path
- **WHEN** the receiver detects multiple folder entries that would map to the same final path after normalization
- **THEN** the transfer SHALL be marked failed instead of producing ambiguous output
