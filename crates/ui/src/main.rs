#[cfg(target_arch = "wasm32")]
fn main() {
    nvsf_ui::run();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!(
        "The frontend is a packaged WASM application. Build it with Trunk in GitHub Actions, then launch nvsf-desktop."
    );
}
