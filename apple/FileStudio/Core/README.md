# Core/

- `CoreAPI.swift`: the contract. Protocols `EngineAPI` and `TabAPI` and plain
  Swift value types (`Row`, `Outcome`, `ShellRequest`, `MenuItem`, `OpStatus`,
  `Settings`, `CoreEvent`…). Views use only this.
- `RustCore.swift`: `RustEngine`/`RustTab`, a pure translation over the UniFFI
  bindings. No decisions.
- `Generated/fstudio_ffi.swift`: written by `scripts/build-core-apple.sh`;
  never edit it by hand.

Need something the contract lacks? Ask the coordinator: the Rust core, the FFI
layer, `CoreAPI.swift`, `RustCore.swift` and `MockCore.swift` change together.
