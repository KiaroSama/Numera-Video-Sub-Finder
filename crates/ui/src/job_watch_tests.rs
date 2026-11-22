use super::*;
use nvsf_core::{Cue, ExtractConfig, MediaInfo};
use std::{
    cell::RefCell,
    collections::VecDeque,
    future::{Future, poll_fn},
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, Waker},
};

#[derive(Default)]
struct Responses {
    queue: VecDeque<Result<Vec<QueueEntry>, String>>,
    status: VecDeque<Result<JobStatus, String>>,
    project: VecDeque<Result<Option<Project>, String>>,
    calls: Vec<&'static str>,
}

struct ControlledIo(Rc<RefCell<Responses>>);

impl JobWatchIo for ControlledIo {
    async fn queue(&mut self) -> Result<Vec<QueueEntry>, String> {
        self.0.borrow_mut().calls.push("queue");
        poll_fn(|_| match self.0.borrow_mut().queue.pop_front() {
            Some(value) => Poll::Ready(value),
            None => Poll::Pending,
        })
        .await
    }

    async fn status(&mut self) -> Result<JobStatus, String> {
        self.0.borrow_mut().calls.push("status");
        poll_fn(|_| match self.0.borrow_mut().status.pop_front() {
            Some(value) => Poll::Ready(value),
            None => Poll::Pending,
        })
        .await
    }

    async fn project(&mut self) -> Result<Option<Project>, String> {
        self.0.borrow_mut().calls.push("project");
        poll_fn(|_| match self.0.borrow_mut().project.pop_front() {
            Some(value) => Poll::Ready(value),
            None => Poll::Pending,
        })
        .await
    }

    async fn wait(&mut self) {}
}

fn poll(future: Pin<&mut impl Future<Output = ()>>) -> Poll<()> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

fn project(text: &str) -> Project {
    Project {
        schema: 1,
        source_hash: format!("source-{text}"),
        config_hash: "config".into(),
        model_hash: "model".into(),
        config: ExtractConfig::default(),
        media: MediaInfo {
            width: 1280,
            height: 720,
            duration_ms: Some(4000),
            codec: "h264".into(),
            time_base_num: 1,
            time_base_den: 1000,
            subtitle_streams: 0,
            rotation: 0,
        },
        cues: vec![Cue::new(1, 0, 100, 500, text.into()).expect("cue")],
        processed_ms: 4000,
        active_appearances: Vec::new(),
        completed: true,
        warnings: Vec::new(),
    }
}

fn completed(id: &str) -> JobStatus {
    JobStatus {
        id: id.into(),
        state: "completed".into(),
        project_ready: true,
        ..JobStatus::default()
    }
}

fn terminal_responses(id: &str) -> Rc<RefCell<Responses>> {
    Rc::new(RefCell::new(Responses {
        queue: VecDeque::from([Ok(Vec::new())]),
        status: VecDeque::from([Ok(completed(id))]),
        ..Responses::default()
    }))
}

fn assert_review_frozen(ui: Ui, text: &str) {
    let revision = ui.revision.get_untracked();
    assert!(!ui.review_editable());
    assert!(!ui.begin_draft());
    ui.edit(|cues| {
        cues[0].text = "Must not replace the pending result".into();
        Ok(())
    });
    ui.travel(false);
    ui.travel(true);
    assert!(!ui.dirty.get_untracked());
    assert!(!ui.draft_dirty.get_untracked());
    assert_eq!(ui.revision.get_untracked(), revision);
    assert_eq!(ui.selected_cue().expect("selected cue").text, text);
}

#[test]
fn terminal_result_loading_blocks_the_next_job_and_review_mutations_in_order() {
    Owner::new().with(|| {
        let ui = Ui::new("dark".into());
        ui.load(project("Before A"));
        let a = ui.begin_review_watch("A".into());
        let a_responses = terminal_responses("A");
        let mut a_io = ControlledIo(a_responses.clone());
        let mut a_watch = std::pin::pin!(ui.watch_job_with(a, &mut a_io));
        assert!(poll(a_watch.as_mut()).is_pending());
        assert_eq!(ui.status.get_untracked().state, "completed");
        assert_eq!(a_responses.borrow().calls, ["queue", "status", "queue"]);
        for command in [
            "start_job",
            "queue_start",
            "pick_project",
            "open_history",
            "pick_checkpoint",
        ] {
            assert!(!ui.request_allowed(command), "{command}");
        }
        ui.start_saved("B".into());
        assert!(!ui.busy.get_untracked());
        assert_eq!(ui.status.get_untracked().id, "A");
        assert_review_frozen(ui, "Before A");

        a_responses.borrow_mut().queue.push_back(Ok(Vec::new()));
        assert!(poll(a_watch.as_mut()).is_pending());
        assert_eq!(a_responses.borrow().calls.last(), Some(&"project"));
        ui.start_saved("B".into());
        assert_review_frozen(ui, "Before A");
        a_responses
            .borrow_mut()
            .project
            .push_back(Ok(Some(project("A"))));
        assert!(poll(a_watch.as_mut()).is_ready());
        assert!(!ui.review_loading.get_untracked());
        assert_eq!(ui.selected_cue().expect("A cue").text, "A");
        assert!(ui.request_allowed("queue_start"));

        let b = ui.begin_review_watch("B".into());
        let b_responses = terminal_responses("B");
        let mut b_io = ControlledIo(b_responses.clone());
        let mut b_watch = std::pin::pin!(ui.watch_job_with(b, &mut b_io));
        assert!(poll(b_watch.as_mut()).is_pending());
        b_responses.borrow_mut().queue.push_back(Ok(Vec::new()));
        assert!(poll(b_watch.as_mut()).is_pending());
        assert_eq!(ui.status.get_untracked().id, "B");
        assert_eq!(ui.status.get_untracked().state, "completed");
        assert_eq!(b_responses.borrow().calls.last(), Some(&"project"));
        assert_review_frozen(ui, "A");
        b_responses
            .borrow_mut()
            .project
            .push_back(Ok(Some(project("B"))));
        assert!(poll(b_watch.as_mut()).is_ready());
        assert!(ui.review_editable());
        assert_eq!(ui.selected_cue().expect("B cue").text, "B");
    });
}

#[test]
fn superseded_watchers_cannot_publish_status_results_errors_or_release_the_new_watch() {
    Owner::new().with(|| {
        for boundary in ["status", "queue", "project", "error"] {
            let ui = Ui::new("dark".into());
            ui.load(project("Original"));
            let a = ui.begin_review_watch("A".into());
            let responses = terminal_responses("A");
            if boundary == "status" || boundary == "error" {
                responses.borrow_mut().status.clear();
            }
            if boundary == "project" {
                responses.borrow_mut().queue.push_back(Ok(Vec::new()));
            }
            let mut io = ControlledIo(responses.clone());
            let mut watch = std::pin::pin!(ui.watch_job_with(a, &mut io));
            assert!(poll(watch.as_mut()).is_pending());
            let b = ui.begin_review_watch("B".into());
            match boundary {
                "status" => responses.borrow_mut().status.push_back(Ok(completed("A"))),
                "error" => responses
                    .borrow_mut()
                    .status
                    .push_back(Err("Old error".into())),
                "queue" => responses.borrow_mut().queue.push_back(Ok(Vec::new())),
                "project" => responses
                    .borrow_mut()
                    .project
                    .push_back(Ok(Some(project("Old A")))),
                _ => unreachable!(),
            }
            assert!(poll(watch.as_mut()).is_ready());
            assert!(ui.current_review_watch(b));
            assert!(ui.review_loading.get_untracked());
            assert_eq!(ui.status.get_untracked().id, "B");
            assert_eq!(ui.status.get_untracked().state, "running");
            assert!(ui.error.get_untracked().is_none());
            assert_review_frozen(ui, "Original");
        }
    });
}
