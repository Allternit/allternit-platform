//! AllternitOS PID 1 library: pure logic (config parsing, service ordering,
//! restart policy). No syscalls here, so the tests run on any host.

pub mod cmdline;
pub mod config;
