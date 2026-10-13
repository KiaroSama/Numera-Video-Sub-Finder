pub mod helpers;

#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(all(target_arch = "wasm32", feature = "e2e"))]
mod e2e;
#[cfg(target_arch = "wasm32")]
mod extract;
#[cfg(target_arch = "wasm32")]
mod ipc;
#[cfg(target_arch = "wasm32")]
mod models;
#[cfg(target_arch = "wasm32")]
mod review;
#[cfg(target_arch = "wasm32")]
mod state;
#[cfg(target_arch = "wasm32")]
mod visual_retry;
#[cfg(target_arch = "wasm32")]
mod workspace;

#[cfg(target_arch = "wasm32")]
pub fn run() {
    app::mount();
}
