use crate::{
    ipc::{Source, Started},
    state::Ui,
};
use leptos::prelude::*;

impl Ui {
    pub(crate) fn replace_source(self, source: Option<Source>) {
        self.replace_source_with(source, |old| {
            #[cfg(target_arch = "wasm32")]
            let _ = web_sys::Url::revoke_object_url(old);
            #[cfg(not(target_arch = "wasm32"))]
            let _ = old;
        });
    }

    pub(crate) fn replace_source_with(self, source: Option<Source>, retire: impl FnOnce(&str)) {
        #[cfg(target_arch = "wasm32")]
        self.studio.reset_source();
        self.preview_generation
            .update(|generation| *generation += 1);
        let old = self.preview_url.get_untracked();
        self.preview_url.set(String::new());
        self.source.set(source);
        self.time.set(0);
        // An outstanding decode owns preview_busy until it completes. Its invalidated
        // generation prevents publishing the old frame and then drains the new source.
        if !old.is_empty() {
            retire(&old);
        }
    }

    pub(crate) fn admit_video(self, source: Source, initial: bool) {
        self.replace_source(Some(source));
        #[cfg(target_arch = "wasm32")]
        {
            if !initial || crate::ipc::e2e_mode() {
                self.seek(0);
                return;
            }
            self.studio.initial.set(true);
            if !self.preview_busy.get_untracked() {
                self.request_initial_frame();
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = initial;
    }

    pub(crate) fn accept_started_source(self, started: Started) -> String {
        self.admit_video(started.source, true);
        self.output.set(Some(started.output));
        started.id
    }
}
