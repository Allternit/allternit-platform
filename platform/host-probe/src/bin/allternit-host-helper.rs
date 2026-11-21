//! `allternit-host-helper`: the one binary the probe runs as root, after the
//! one-time admin setup. It only does what `host_probe::helper` allows and
//! re-checks every request itself. See `host_probe::elevation`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(host_probe::helper::main(args));
}
