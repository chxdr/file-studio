# Mock/

`MockCore.swift` implements `EngineAPI`/`TabAPI` with no Rust
(`-FSMockCore`): a synthetic tree under `/Mock` with a 100k-entry folder,
unicode and case-twin names, iCloud placeholders and packages; a fake queue
with progress and a conflict on paste; completion over children plus a small
frecency list; the common Standard keys.

It imitates the core so views have something to draw. It is not a spec:
never copy behaviour from here into the app.
