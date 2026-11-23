use crate::{
    ipc::{self, JobStatus, QueueEntry},
    state::Ui,
};
use leptos::prelude::*;
use nvsf_core::Project;
use serde_json::json;

pub(crate) trait JobWatchIo {
    async fn queue(&mut self) -> Result<Vec<QueueEntry>, String>;
    async fn status(&mut self) -> Result<JobStatus, String>;
    async fn project(&mut self) -> Result<Option<Project>, String>;
    async fn wait(&mut self);
}

pub(crate) struct NativeJobWatch;

impl JobWatchIo for NativeJobWatch {
    async fn queue(&mut self) -> Result<Vec<QueueEntry>, String> {
        ipc::call("queue_list", json!({})).await
    }

    async fn status(&mut self) -> Result<JobStatus, String> {
        ipc::call("job_status", json!({})).await
    }

    async fn project(&mut self) -> Result<Option<Project>, String> {
        ipc::call("current_project", json!({})).await
    }

    async fn wait(&mut self) {
        gloo_timers::future::TimeoutFuture::new(600).await;
    }
}

impl Ui {
    pub(crate) fn begin_review_watch(self, id: String) -> u64 {
        self.review_generation.update(|generation| *generation += 1);
        self.review_loading.set(true);
        self.status.set(JobStatus {
            id,
            state: "running".into(),
            ..JobStatus::default()
        });
        self.route.set("Queue".into());
        self.review_generation.get_untracked()
    }

    pub(crate) fn current_review_watch(self, generation: u64) -> bool {
        !self.review_generation.is_disposed()
            && self.review_generation.get_untracked() == generation
    }

    pub(crate) async fn watch_job_with(self, generation: u64, io: &mut impl JobWatchIo) {
        let queue = io.queue().await;
        if !self.current_review_watch(generation) {
            return;
        }
        match queue {
            Ok(entries) => self.queue.set(entries),
            Err(error) => self.error.set(Some(error)),
        }
        loop {
            io.wait().await;
            if !self.current_review_watch(generation) {
                return;
            }
            let status = io.status().await;
            if !self.current_review_watch(generation) {
                return;
            }
            match status {
                Ok(status) => {
                    let active =
                        matches!(status.state.as_str(), "running" | "paused" | "cancelling");
                    if let Some(error) = status.error.clone() {
                        self.error.set(Some(error));
                    }
                    let ready = status.project_ready;
                    self.status.set(status);
                    if !active {
                        let queue = io.queue().await;
                        if !self.current_review_watch(generation) {
                            return;
                        }
                        match queue {
                            Ok(entries) => self.queue.set(entries),
                            Err(error) => self.error.set(Some(error)),
                        }
                        if ready {
                            loop {
                                let project = io.project().await;
                                if !self.current_review_watch(generation) {
                                    return;
                                }
                                match project {
                                    Ok(Some(project)) => {
                                        self.load(project);
                                        break;
                                    }
                                    Ok(None) => self.error.set(Some(
                                        "Completed job did not return a review project".into(),
                                    )),
                                    Err(error) => self.error.set(Some(error)),
                                }
                                io.wait().await;
                                if !self.current_review_watch(generation) {
                                    return;
                                }
                            }
                        }
                        break;
                    }
                }
                Err(error) => {
                    self.error.set(Some(error));
                }
            }
        }
        if self.current_review_watch(generation) {
            self.review_loading.set(false);
        }
    }
}

#[cfg(test)]
#[path = "job_watch_tests.rs"]
mod tests;
