//! Effect connectors for the task types' write-set schemes (WP-C3a/C3b).
//! Each connector runs only inside P1's fenced, idempotent effect path
//! (`executor::effect_with`), after the POLICY node authorized the write set.

// ── WP-C3b: computer: (COMPUTER_USE) and campaign: (CAMPAIGN) ──
pub mod campaign;
pub mod computer;
#[cfg(test)]
mod tests_c3b;
// ── end WP-C3b ──
