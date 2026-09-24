//! Native AetherStack desktop entry point.

fn main() {
    if let Err(error) = aetherstack_desktop_lib::run() {
        eprintln!("AetherStack desktop terminated with an error: {error}");
        std::process::exit(1);
    }
}
