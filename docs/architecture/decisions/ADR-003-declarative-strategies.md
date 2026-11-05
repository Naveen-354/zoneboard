# ADR-003: strategies stay declarative; no user code runs in ZoneBoard
Status: accepted (roadmap D6). The master prompt allows custom code only in an isolated, resource-limited
sandbox. Building such a sandbox safely on Windows is a project of its own; until then strategies are validated
JSON (with an SDK, import/export and packs for developers), and the AI assistant can only draft that JSON.
