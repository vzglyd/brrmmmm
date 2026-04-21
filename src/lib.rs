#![deny(missing_docs)]
//! `brrmmmm` is the Rust runtime and inspection library for portable Wasm
//! mission modules.
//!
//! The supported public API for this crate is intentionally narrow:
//!
//! - [`abi`] exposes the mission-module describe schema and runtime snapshot types.
//! - [`config`] loads runtime configuration and resource limits.
//! - [`controller`] loads, validates, inspects, and runs mission modules.
//! - [`error`] defines structured runtime and configuration failures.
//! - [`events`] defines the structured event stream emitted by the runtime.
//!
//! The `brrmmmm` binary is the primary integration surface for operators,
//! orchestrators, and other non-Rust callers. This library exists so the CLI,
//! the TUI, and the test suite share one runtime implementation, while still
//! allowing narrow Rust integrations when needed.
//!
//! Lower-level runtime machinery such as host import registration, persistence
//! internals, attestation helpers, and browser/network session wiring is
//! intentionally not part of the supported public API and may change without
//! notice.
//!
//! # Legal And Ethical Use
//!
//! `brrmmmm` can execute mission modules that automate network access, browser login
//! flows, and CAPTCHA remediation. The project does not grant authorization,
//! waive third-party Terms of Service, or determine whether a given workflow is
//! lawful in a particular jurisdiction.
//!
//! Legal compliance, contractual compliance, target-service authorization, and
//! operator review remain the sole responsibility of the mission-module author and the
//! party deploying or running the mission. This crate documents runtime
//! capabilities only and does not provide legal advice.

pub mod abi;
mod attestation;
pub mod config;
pub mod controller;
pub mod error;
pub mod events;
mod host;
mod identity;
mod mission_ledger;
mod mission_state;
mod persistence;
mod utils;
