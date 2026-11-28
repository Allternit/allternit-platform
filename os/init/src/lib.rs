//! AllternitOS PID 1 library: pure logic (config parsing, service ordering,
//! restart policy, dm-verity, A/B slot state, LUKS2 header). No syscalls here, so the
//! tests run on any host.

pub mod config;
pub mod luks;
pub mod slot;
pub mod verity;
