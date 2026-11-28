//! AllternitOS PID 1. Linux only. Real compile and boot happen in CI (plan step 27).

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("allternit-init runs on Linux only");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
fn main() {
    linux::run();
}
