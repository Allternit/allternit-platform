//! Factory part `orchestration`: mail, bus, attention, steering, observer (SPEC §5).
//! `send` is the one send path and its `Delivery` records.

pub mod attention;
pub mod bus;
pub mod mail;
pub mod observer;
pub mod send;
pub mod steer;
