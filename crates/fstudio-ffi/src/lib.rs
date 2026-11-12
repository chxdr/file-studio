//! UniFFI exports only: thin wrappers over `fstudio-core` (F-004). The real API lands in M1/M2.

uniffi::setup_scaffolding!();

/// The core's version.
#[uniffi::export]
pub fn core_version() -> String {
    fstudio_core::version().to_string()
}
