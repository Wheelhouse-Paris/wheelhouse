# Changelog — wh-proto

All notable changes to the Wheelhouse Protobuf schema definitions.

Published crate name: `wheelhouse-proto` (TT-03)

## [0.1.0] — 2026-03-12

### Added

- Initial schema: `core.proto` (TextMessage, FileMessage, Reaction)
- Initial schema: `skills.proto` (SkillInvocation, SkillResult, SkillProgress)
- Initial schema: `system.proto` (CronEvent, TopologyShutdown, StreamCapacityWarning)
- Initial schema: `stream.proto` (StreamEnvelope, TypeRegistration, TypeRegistryEntry)
- Reserved field blocks in all messages for future evolution (TT-02)
- Proto fixture files for backward-compatibility testing (NFR-E1)
