//! Session Negotiator (spec §6): pure functions that turn what the client
//! advertised and what the host is into a session plan, with a reason for
//! every decision. No macOS calls, no I/O — everything here is unit-tested.

pub mod display;
pub mod handler;
pub mod host;
pub mod session;
pub mod video;
