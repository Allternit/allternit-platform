//! Print one host snapshot as JSON: `cargo run -p host-probe --example snapshot`.

fn main() {
    match host_probe::snapshot() {
        Ok(s) => println!(
            "{}",
            serde_json::to_string_pretty(&s).expect("HostSnapshot serializes")
        ),
        Err(e) => {
            eprintln!("host-probe: {e}");
            std::process::exit(1);
        }
    }
}
