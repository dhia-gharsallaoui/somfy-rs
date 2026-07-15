#![cfg_attr(not(test), no_std)]

mod types;

pub use types::{Direction, DomainError, Pos, ShadeConfig, ShadeKind, TiltMode};
