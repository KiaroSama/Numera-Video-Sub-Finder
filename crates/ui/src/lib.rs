pub mod helpers;

#[cfg(target_arch = "wasm32")]
mod close_modal;
#[cfg(any(target_arch = "wasm32", test))]
mod hotkeys;
#[cfg(target_arch = "wasm32")]
mod player;
#[cfg(target_arch = "wasm32")]
mod roi_editor;
#[cfg(any(target_arch = "wasm32", test))]
mod roi_geometry;
#[cfg(target_arch = "wasm32")]
mod studio_state;
#[cfg(target_arch = "wasm32")]
mod toolbar;
#[cfg(any(target_arch = "wasm32", test))]
mod undo;
#[cfg(target_arch = "wasm32")]
mod update_banner;

#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(all(target_arch = "wasm32", feature = "e2e"))]
mod e2e;
#[cfg(target_arch = "wasm32")]
mod evidence;
#[cfg(target_arch = "wasm32")]
mod extract;
#[cfg(target_arch = "wasm32")]
mod glimm;
#[cfg(any(target_arch = "wasm32", test))]
mod ipc;
#[cfg(any(target_arch = "wasm32", test))]
mod job_watch;
#[cfg(target_arch = "wasm32")]
mod languages;
#[cfg(any(target_arch = "wasm32", test))]
mod maintenance;
#[cfg(target_arch = "wasm32")]
mod models;
#[cfg(target_arch = "wasm32")]
mod review;
#[cfg(any(target_arch = "wasm32", test))]
mod state;
#[cfg(target_arch = "wasm32")]
mod visual_retry;
#[cfg(target_arch = "wasm32")]
mod workspace;

#[cfg(target_arch = "wasm32")]
pub fn run() {
    app::mount();
}
