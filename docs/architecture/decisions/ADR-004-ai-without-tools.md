# ADR-004: the AI assistant gets no tools
Status: accepted. Instead of a tool-permission system that must be audited forever, the assistant receives
read-only context and returns text or JSON, so it cannot act at all. Approvals require actor "user" and a typed
phrase; drafts pass the strict validator; explanations are checked number by number against stored results.
