use crate::{
    files,
    state::{Host, Result, blocking, lock},
};
use nvsf_core::Project;
use serde::Deserialize;
use std::{
    sync::{Mutex, atomic::Ordering},
    time::Duration,
};
use tauri::{Emitter, EventTarget, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

#[derive(Default)]
pub(crate) struct CloseSession(Mutex<Option<ReviewState>>);

#[derive(Debug, PartialEq)]
enum CloseAction {
    Save,
    Discard,
    Cancel,
}

impl CloseAction {
    fn parse(action: &str) -> Result<Self> {
        match action {
            "save" => Ok(Self::Save),
            "discard" => Ok(Self::Discard),
            "cancel" => Ok(Self::Cancel),
            _ => Err("Unknown close action; choose save, discard or cancel".into()),
        }
    }
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ReviewState {
    dirty: bool,
    draft: bool,
    revision: u64,
    busy: bool,
}

fn decode(text: &str) -> Result<ReviewState> {
    if text.len() > 4096 {
        return Err("Review close acknowledgement exceeds its size bound".into());
    }
    let state: ReviewState = serde_json::from_str(text)
        .map_err(|_| "Review close acknowledgement is missing or invalid".to_string())?;
    if state.busy {
        return Err("A review operation is still pending; wait before closing".into());
    }
    Ok(state)
}

fn unchanged(before: &ReviewState, after: &ReviewState) -> Result<()> {
    if before != after {
        return Err("Review changed during close; return to review before closing again".into());
    }
    Ok(())
}

fn review_eval(window: &tauri::WebviewWindow, script: &str) -> Result<String> {
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    window
        .eval_with_callback(script, move |text| {
            let _ = send.try_send(text);
        })
        .map_err(|error| error.to_string())?;
    receive
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| "Review did not acknowledge close within two seconds".to_string())
}

fn snapshot(window: &tauri::WebviewWindow) -> Result<ReviewState> {
    decode(&review_eval(
        window,
        "(()=>{try{if(typeof window.__NVSF_REVIEW_STATE__!=='function')return null;if(!window.__NVSF_CLOSE_LOCK__){const target=document.querySelector('.shell')||document.body;window.__NVSF_CLOSE_LOCK__={target,inert:target.inert};target.inert=true;}return window.__NVSF_REVIEW_STATE__(true);}catch(_){return null;}})()",
    )?)
}

fn unlock(window: &tauri::WebviewWindow, host: &Host) {
    if let Err(error) = window.eval("if(window.__NVSF_CLOSE_LOCK__){const {target,inert}=window.__NVSF_CLOSE_LOCK__;target.inert=inert;delete window.__NVSF_CLOSE_LOCK__;}if(typeof window.__NVSF_REVIEW_STATE__==='function')window.__NVSF_REVIEW_STATE__(false);") {
        host.0.log.event("ERROR", "DESKTOP", &format!("Unable to restore review interaction after deferred close: {error}"));
    }
}

#[cfg(any(test, feature = "e2e"))]
fn authorize(
    mut read: impl FnMut() -> Result<ReviewState>,
    discard: impl FnOnce() -> bool,
    shutdown: impl FnOnce() -> bool,
) -> Result<bool> {
    let before = read()?;
    if (before.dirty || before.draft) && !discard() {
        return Ok(false);
    }
    unchanged(&before, &read()?)?;
    Ok(shutdown())
}

struct CloseRecovery<'a> {
    window: &'a tauri::WebviewWindow,
    host: &'a Host,
    armed: bool,
}

impl Drop for CloseRecovery<'_> {
    fn drop(&mut self) {
        if self.armed {
            unlock(self.window, self.host);
            self.host.0.closing.store(false, Ordering::Release);
        }
    }
}

fn finish(app: &tauri::AppHandle, recovery: &mut CloseRecovery<'_>, restart: bool) -> Result<()> {
    let host = recovery.host;
    if !host.shutdown() {
        return Err("An extraction, model or update operation is still stopping. Wait for it to finish, then try again.".into());
    }
    host.0.exit_approved.store(true, Ordering::Release);
    host.0
        .log
        .event("INFO", "DESKTOP", "phase=shutdown-complete");
    recovery.armed = false;
    if restart {
        app.restart();
    }
    app.exit(0);
    Ok(())
}

pub fn request(app: &tauri::AppHandle, host: &Host) {
    let app = app.clone();
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = request_exit(app, host.clone(), false).await {
            host.0.log.event("WARNING", "DESKTOP", &error);
        }
    });
}

pub(crate) async fn request_exit(app: tauri::AppHandle, host: Host, restart: bool) -> Result<()> {
    if !host.wait_to_close(Duration::from_secs(2)).await? {
        return if restart {
            Err("A close request is already active".into())
        } else {
            Ok(())
        };
    }
    blocking(move || begin_request(&app, &host, restart)).await
}

fn begin_request(app: &tauri::AppHandle, host: &Host, restart: bool) -> Result<()> {
    let Some(window) = app.get_webview_window("main") else {
        host.0.closing.store(false, Ordering::Release);
        return Err("Main review window unavailable; close refused".into());
    };
    let mut recovery = CloseRecovery {
        window: &window,
        host,
        armed: true,
    };
    let before = snapshot(&window)?;
    if before.dirty || before.draft || host.0.unsaved.load(Ordering::Acquire) {
        if restart {
            return Err("Save or discard the current corrections before restarting".into());
        }
        if review_eval(&window, "window.__NVSF_CLOSE_READY__ === true")
            .is_ok_and(|ready| ready == "true")
        {
            let session = app.state::<CloseSession>();
            let mut pending = lock(&session.0)?;
            if app
                .emit_to(
                    EventTarget::webview_window("main"),
                    "nvsf://close-requested",
                    (),
                )
                .is_ok()
            {
                *pending = Some(before);
                recovery.armed = false;
                return Ok(());
            }
        }
        if !app
            .dialog()
            .message("Your review has unsaved changes. Return to save them, or discard and close?")
            .title("Unsaved review changes")
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Discard and close".into(),
                "Return to review".into(),
            ))
            .blocking_show()
        {
            return Ok(());
        }
    }
    verify_snapshot(&window, &before)?;
    if restart && host.0.unsaved.load(Ordering::Acquire) {
        return Err("Save or discard the current corrections before restarting".into());
    }
    let result = finish(app, &mut recovery, restart);
    if !restart && let Err(error) = &result {
        app.dialog()
            .message(error)
            .title("Worker shutdown pending")
            .blocking_show();
    }
    result
}

fn verify_snapshot(window: &tauri::WebviewWindow, before: &ReviewState) -> Result<()> {
    let after = review_eval(
        window,
        "(()=>{try{return window.__NVSF_CLOSE_LOCK__?.target.isConnected && window.__NVSF_CLOSE_LOCK__.target.inert ? window.__NVSF_REVIEW_STATE__(true) : null;}catch(_){return null;}})()",
    )?;
    unchanged(before, &decode(&after)?)
}

#[tauri::command]
pub async fn confirm_close(
    app: tauri::AppHandle,
    host: tauri::State<'_, Host>,
    action: String,
    project: Option<Project>,
) -> Result<()> {
    let action = CloseAction::parse(&action)?;
    let host = host.inner().clone();
    blocking(move || {
        let session = app.state::<CloseSession>();
        let before = lock(&session.0)?
            .take()
            .filter(|_| host.0.closing.load(Ordering::Acquire))
            .ok_or_else(|| "No active close confirmation".to_string())?;
        let Some(window) = app.get_webview_window("main") else {
            host.0.closing.store(false, Ordering::Release);
            return Err("Main review window unavailable; close refused".into());
        };
        let mut recovery = CloseRecovery {
            window: &window,
            host: &host,
            armed: true,
        };
        if action == CloseAction::Cancel {
            return Ok(());
        }
        verify_snapshot(&window, &before)?;
        if action == CloseAction::Save {
            if before.draft {
                return Err(
                    "Apply or discard the current cue draft before saving and exiting".into(),
                );
            }
            if let Some(project) = project {
                let _operation = lock(&host.0.operation)?;
                files::apply_project_edits(&host, project)?;
                host.0.unsaved.store(true, Ordering::Release);
            } else if before.dirty {
                return Err("Send the current review corrections before saving and exiting".into());
            }
            let verify = || verify_snapshot(&window, &before);
            if files::save_active_project(&app, &host, false, Some(&verify))?.is_none() {
                return Ok(());
            }
        }
        verify_snapshot(&window, &before)?;
        finish(&app, &mut recovery, false)
    })
    .await
}

#[cfg(feature = "e2e")]
pub fn probe(window: &tauri::WebviewWindow, host: &Host) -> Result<serde_json::Value> {
    let result = (|| {
        let before = snapshot(window)?;
        window.eval("for(const key of ['z','s'])window.dispatchEvent(new KeyboardEvent('keydown',{key,ctrlKey:true,bubbles:true,cancelable:true}));")
            .map_err(|error| error.to_string())?;
        unchanged(&before, &snapshot(window)?)?;
        if authorize(|| snapshot(window), || false, || false)? {
            return Err("Close probe must never authorize process exit".into());
        }
        Ok(
            serde_json::json!({"dirty":before.dirty,"draft":before.draft,"revision":before.revision,"busy":before.busy}),
        )
    })();
    unlock(window, host);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_actions_accept_only_the_three_modal_choices() {
        assert_eq!(CloseAction::parse("save"), Ok(CloseAction::Save));
        assert_eq!(CloseAction::parse("discard"), Ok(CloseAction::Discard));
        assert_eq!(CloseAction::parse("cancel"), Ok(CloseAction::Cancel));
        for invalid in ["", "Save", "discard ", "restart"] {
            assert!(CloseAction::parse(invalid).is_err());
        }
    }

    #[test]
    fn close_authorization_preserves_late_edits_and_pending_operations() {
        use std::cell::Cell;
        let clean = r#"{"dirty":false,"draft":false,"revision":4,"busy":false}"#;
        let dirty = r#"{"dirty":true,"draft":false,"revision":5,"busy":false}"#;
        let draft = r#"{"dirty":false,"draft":true,"revision":4,"busy":false}"#;
        let busy = r#"{"dirty":false,"draft":false,"revision":4,"busy":true}"#;
        for (first, second, discard, expected) in [
            (clean, clean, false, Some(true)),
            (clean, dirty, false, None),
            (clean, draft, false, None),
            (clean, busy, false, None),
            (clean, "null", false, None),
            ("{}", clean, true, None),
            (busy, clean, true, None),
            (dirty, dirty, false, Some(false)),
            (draft, draft, false, Some(false)),
            (dirty, dirty, true, Some(true)),
            (draft, draft, true, Some(true)),
            (dirty, clean, true, None),
        ] {
            let mut responses = [first, second].into_iter();
            let prompts = Cell::new(0);
            let shutdowns = Cell::new(0);
            let result = authorize(
                || decode(responses.next().expect("at most two acknowledgements")),
                || {
                    prompts.set(prompts.get() + 1);
                    discard
                },
                || {
                    shutdowns.set(shutdowns.get() + 1);
                    true
                },
            );
            assert_eq!(result.ok(), expected, "{first} -> {second}");
            assert_eq!(shutdowns.get(), u32::from(expected == Some(true)));
            assert_eq!(prompts.get(), u32::from(first == dirty || first == draft));
        }
        assert!(
            !authorize(
                || decode(clean),
                || panic!("clean review needs no discard prompt"),
                || false
            )
            .unwrap()
        );
    }

    #[test]
    fn missing_invalid_and_pending_review_cannot_authorize_exit() {
        for text in [
            "null",
            "{}",
            "true",
            r#"{"dirty":false,"draft":false,"revision":-1,"busy":false}"#,
            r#"{"dirty":false,"draft":false,"revision":0,"busy":true}"#,
            r#"{"dirty":false,"draft":false,"revision":0,"busy":false,"other":true}"#,
        ] {
            assert!(decode(text).is_err(), "{text}");
        }
        assert!(decode(&" ".repeat(4097)).is_err());
        let clean = decode(r#"{"dirty":false,"draft":false,"revision":4,"busy":false}"#).unwrap();
        assert!(unchanged(&clean, &clean).is_ok());
        for text in [
            r#"{"dirty":true,"draft":false,"revision":4,"busy":false}"#,
            r#"{"dirty":false,"draft":true,"revision":4,"busy":false}"#,
            r#"{"dirty":false,"draft":false,"revision":5,"busy":false}"#,
        ] {
            assert!(unchanged(&clean, &decode(text).unwrap()).is_err());
        }
        let dirty = decode(r#"{"dirty":true,"draft":true,"revision":4,"busy":false}"#).unwrap();
        assert!(dirty.dirty && dirty.draft);
        assert!(unchanged(&dirty, &dirty).is_ok());
        assert!(unchanged(&dirty, &clean).is_err());
    }
}
