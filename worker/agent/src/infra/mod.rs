//! Infrastructure layer — OS interactions and side-effect boundary.
//!
//! Provides process lifecycle management, system metrics collection,
//! file operations, and time utilities. Platform-specific code is isolated here.
pub mod process;
pub mod system;
pub mod time;
