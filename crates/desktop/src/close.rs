use crate::state::{Host, Result};
use serde::Deserialize;
use std::{sync::atomic::Ordering, time::Duration};
use tauri::Manager;

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

fn snapshot(window: &tauri::WebviewWindow) -> Result<ReviewState> {
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    window
        .eval_with_callback(
            "(()=>{try{if(typeof window.__NVSF_REVIEW_STATE__!=='function')return null;if(!window.__NVSF_CLOSE_LOCK__)window.__NVSF_CLOSE_LOCK__={inert:document.body.inert};document.body.inert=true;return window.__NVSF_REVIEW_STATE__(true);}catch(_){return null;}})()",
            move |text| { let _ = send.try_send(text); },
        )
        .map_err(|error| error.to_string())?;
    let text = receive
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| "Review did not acknowledge close within two seconds".to_string())?;
    decode(&text)
}

fn unlock(window: &tauri::WebviewWindow, host: &Host) {
    if let Err(error) = window.eval("if(window.__NVSF_CLOSE_LOCK__){document.body.inert=window.__NVSF_CLOSE_LOCK__.inert;delete window.__NVSF_CLOSE_LOCK__;}if(typeof window.__NVSF_REVIEW_STATE__==='function')window.__NVSF_REVIEW_STATE__(false);") {
        host.0.log.event("ERROR", "DESKTOP", &format!("Unable to restore review interaction after deferred close: {error}"));
    }
}

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

pub fn request(app: &tauri::AppHandle, host: &Host) {
    let app = app.clone();
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        match host.wait_to_close(Duration::from_secs(2)).await {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                host.0.log.event("WARNING", "DESKTOP", &error);
                return;
            }
        }
        let completion = host.clone();
        let recovery_app = app.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let Some(window) = app.get_webview_window("main") else {
                host.0.closing.store(false, Ordering::Release);
                host.0.log.event("ERROR", "DESKTOP", "Main review window unavailable; close refused");
                return;
            };
            let attempt = (|| -> Result<bool> {
                use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
                if !authorize(
                    || snapshot(&window),
                    || app.dialog().message("Your review has unsaved changes. Return to save them, or discard and close?")
                        .title("Unsaved review changes").buttons(MessageDialogButtons::OkCancelCustom("Discard and close".into(), "Return to review".into())).blocking_show(),
                    || {
                        if host.shutdown() {
                            return true;
                        }
                        app.dialog().message("An extraction or model operation is still stopping. The application remains open to preserve checkpoints and resources. Wait for cancellation to finish, then close again.").title("Worker shutdown pending").blocking_show();
                        false
                    },
                )? {
                    return Ok(false);
                }
                host.0.exit_approved.store(true, Ordering::Release);
                host.0.log.event("INFO", "DESKTOP", "phase=shutdown-complete");
                app.exit(0);
                Ok(true)
            })();
            if !matches!(attempt, Ok(true)) {
                if let Err(error) = attempt { host.0.log.event("WARNING", "DESKTOP", &error); }
                unlock(&window, &host);
                host.0.closing.store(false, Ordering::Release);
            }
        }).await;
        if result.is_err() {
            completion.0.log.event(
                "ERROR",
                "DESKTOP",
                "Native close task stopped unexpectedly; application remains open",
            );
            if let Some(window) = recovery_app.get_webview_window("main") {
                unlock(&window, &completion);
            }
            completion.0.closing.store(false, Ordering::Release);
        }
    });
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
