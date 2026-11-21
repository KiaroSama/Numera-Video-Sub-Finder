//! Standalone CI entrypoint for tracked source publication and UTF-8 policy.

#[path = "../../xtask/src/source_policy.rs"]
mod source_policy;

fn main() {
    if let Err(err) = source_policy::verify() {
        eprintln!("[ERROR] [SOURCE-POLICY] {err}");
        std::process::exit(1);
    }
}
