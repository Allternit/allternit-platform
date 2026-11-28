//! AllternitOS PID 1 library: pure logic (config parsing, service ordering,
//! restart policy, dm-verity, A/B slot state, LUKS2 header, signed manifest, Secure Boot
//! variable). No syscalls here, so the tests run on any host.

pub mod config;
pub mod luks;
pub mod manifest;
pub mod secureboot;
pub mod slot;
pub mod verity;
