#![deny(missing_docs)]
//! `brrmmmm` is the Rust runtime and inspection library for VZGLYD sidecar WASM
//! modules.
//!
//! The supported public API for this crate is intentionally narrow:
//!
//! - [`abi`] exposes the sidecar describe schema and runtime snapshot types.
//! - [`config`] loads runtime configuration and resource limits.
//! - [`controller`] loads, validates, inspects, and runs sidecar modules.
//! - [`error`] defines structured runtime and configuration failures.
//! - [`events`] defines the structured event stream emitted by the runtime.
//!
//! # Legal And Ethical Use
//!
//! `brrmmmm` can execute sidecars that automate network access, browser login
//! flows, and CAPTCHA remediation. The project does not grant authorization,
//! waive third-party Terms of Service, or determine whether a given workflow is
//! lawful in a particular jurisdiction.
//!
//! Legal compliance, contractual compliance, target-service authorization, and
//! operator review remain the sole responsibility of the sidecar author and the
//! party deploying or running the sidecar. This crate documents runtime
//! capabilities only and does not provide legal advice.

pub mod abi;
mod attestation;
pub mod config;
pub mod controller;
pub mod error;
pub mod events;
mod host;
mod identity;
mod mission_state;
mod persistence;
mod utils;
