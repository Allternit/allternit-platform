//! Dev/test harness for the pane engine (never shipped). The product runs the
//! same entry point as `allternit-factory pane …`.

fn main() -> std::io::Result<()> {
    allternit_factory_pane::run(std::env::args_os())
}
