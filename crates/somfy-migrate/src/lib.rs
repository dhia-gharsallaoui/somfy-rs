#![cfg_attr(not(test), no_std)]
//! # somfy-migrate
//!
//! Zero-copy value tokenizer for the C++ ESPSomfy-RTS `ConfigFile` text format,
//! the first layer of migrating existing device backups into somfy-rs.
//!
//! The C++ reference (`src/ConfigFile.cpp`) serializes every record as a line of
//! comma-separated, space-padded fields terminated by `\n`. [`Reader`] walks an
//! in-memory `&[u8]` with the same field/terminator/quoting rules as the C++
//! read primitives, but never allocates and uses no floating-point types (a
//! workspace constraint — fractional positions are parsed straight into integer
//! hundredths by [`Reader::read_f32_as_centi`]).
//!
//! ## Port fidelity
//!
//! Every primitive mirrors a specific C++ function and cites it in its doc
//! comment. Where this crate deliberately diverges from the C++ (EOF handling,
//! buffer-overflow handling, the `writeBool` format), the divergence is called
//! out at [`MigrateError`] and on the affected method — the C++ behavior is the
//! reference, but a migrator surfaces corruption rather than silently
//! substituting defaults the way the on-device reader does.

mod reader;

pub use reader::{MigrateError, Reader};
