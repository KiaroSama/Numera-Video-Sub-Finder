#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    OpenVideo,
    OpenProject,
    Save,
    SaveAs,
    Undo,
    Redo,
    Play,
    Step(i64),
    Frame(i8),
    SetStart,
    SetEnd,
    DeleteRegion,
    Zoom(i8),
    Extract,
    Help,
    Escape,
}

fn shortcut(key: &str, control: bool, shift: bool, editing: bool) -> Option<Action> {
    use Action::*;
    match (key, control, shift) {
        ("Escape", _, _) => Some(Escape),
        ("F1", false, _) => Some(Help),
        ("o" | "O", true, false) => Some(OpenVideo),
        ("o" | "O", true, true) => Some(OpenProject),
        ("s" | "S", true, false) => Some(Save),
        ("s" | "S", true, true) => Some(SaveAs),
        _ if editing => None,
        ("z" | "Z", true, false) => Some(Undo),
        ("z" | "Z", true, true) | ("y" | "Y", true, false) => Some(Redo),
        (" ", false, false) => Some(Play),
        ("ArrowLeft", false, false) => Some(Frame(-1)),
        ("ArrowRight", false, false) => Some(Frame(1)),
        ("ArrowLeft", false, true) => Some(Step(-1000)),
        ("ArrowRight", false, true) => Some(Step(1000)),
        ("i" | "I", false, false) => Some(SetStart),
        ("o" | "O", false, false) => Some(SetEnd),
        ("Delete" | "Backspace", false, false) => Some(DeleteRegion),
        ("=" | "+", true, _) => Some(Zoom(1)),
        ("-", true, false) => Some(Zoom(-1)),
        ("0", true, false) => Some(Zoom(0)),
        ("Enter", true, false) => Some(Extract),
        _ => None,
    }
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::{Action, shortcut};
    use crate::{close_modal::StudioDialog, state::Ui};
    use leptos::{ev, prelude::*};
    use wasm_bindgen::JsCast;

    pub(crate) fn register(ui: Ui) {
        let handle = window_event_listener(ev::keydown, move |event| {
            if event.default_prevented()
                || event.is_composing()
                || event.key_code() == 229
                || event.alt_key()
            {
                return;
            }
            let key = event.key();
            let target = event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok());
            let editing = target.as_ref().is_some_and(|e| {
                e.closest("input, textarea, select")
                    .ok()
                    .flatten()
                    .is_some()
                    || e.dyn_ref::<web_sys::HtmlElement>()
                        .is_some_and(web_sys::HtmlElement::is_content_editable)
            });
            let activate = key == " "
                && target.as_ref().is_some_and(|e| {
                    e.closest("button, summary, a, [role=button]")
                        .ok()
                        .flatten()
                        .is_some()
                });
            let Some(action) = shortcut(
                &key,
                event.ctrl_key() || event.meta_key(),
                event.shift_key(),
                editing || activate,
            ) else {
                return;
            };
            if ui.studio.close_open.get_untracked() {
                // Native dialog handles Escape while the close lock is held.
                if !matches!(action, Action::Escape) {
                    event.prevent_default();
                }
                return;
            }
            if ui.closing.get_untracked()
                || web_sys::window().is_some_and(|w| {
                    js_sys::Reflect::has(&w, &"__NVSF_CLOSE_LOCK__".into()).unwrap_or(true)
                })
            {
                return;
            }
            if ui.studio.shortcuts.get_untracked()
                && !matches!(action, Action::Help | Action::Escape)
            {
                event.prevent_default();
                return;
            }
            let extract = ui.route.get_untracked() == "Extract";
            let preview = matches!(ui.route.get_untracked().as_str(), "Extract" | "Review");
            if event.repeat()
                && !matches!(action, Action::Step(_) | Action::Frame(_) | Action::Zoom(_))
            {
                event.prevent_default();
                return;
            }
            match action {
                Action::OpenVideo => ui.open_video(),
                Action::OpenProject => ui.open_project(),
                Action::Save => ui.save(false),
                Action::SaveAs => ui.save(true),
                Action::Undo => ui.undo(),
                Action::Redo => ui.redo(),
                Action::Play if preview => ui.toggle_play(),
                Action::Step(delta) if preview => ui.step(delta),
                Action::Frame(direction) if preview => ui.step_frame(direction),
                Action::SetStart if extract => ui.mark_range(false),
                Action::SetEnd if extract => ui.mark_range(true),
                Action::DeleteRegion if extract && ui.remove_selected_region() => {}
                Action::Zoom(delta) if preview => ui.change_zoom(delta),
                Action::Extract if extract && ui.can_extract() => ui.start_extraction(),
                Action::Help => ui.studio.shortcuts.update(|open| *open = !*open),
                Action::Escape if ui.studio.shortcuts.get_untracked() => {
                    ui.studio.shortcuts.set(false)
                }
                Action::Escape if extract && ui.studio.roi_drag.get_untracked().is_some() => {
                    ui.studio.roi_drag.set(None)
                }
                _ => return,
            }
            event.prevent_default();
        });
        on_cleanup(move || handle.remove());
    }

    #[component]
    pub(crate) fn ShortcutsModal(ui: Ui) -> impl IntoView {
        let shortcuts = [
            ("Open video", "Ctrl+O"),
            ("Open project", "Ctrl+Shift+O"),
            ("Save", "Ctrl+S"),
            ("Save As", "Ctrl+Shift+S"),
            ("Undo", "Ctrl+Z"),
            ("Redo", "Ctrl+Y / Ctrl+Shift+Z"),
            ("Play / Pause", "Space"),
            ("Previous / next frame", "← / →"),
            ("Jump by one second", "Shift+← / Shift+→"),
            ("Set range start / end", "I / O"),
            ("Delete selected region", "Delete / Backspace"),
            ("Zoom in / out / Fit", "Ctrl+= / Ctrl+- / Ctrl+0"),
            ("Extract subtitles", "Ctrl+Enter"),
            ("Keyboard shortcuts", "F1"),
            ("Close dialog or cancel drawing", "Esc"),
            ("Move focus", "Tab / Shift+Tab"),
        ];
        view! {
            <StudioDialog id="shortcuts-modal" label="shortcuts-title" role="dialog"
                visible=ui.studio.shortcuts on_cancel=move ||ui.studio.shortcuts.set(false)>
                <h2 id="shortcuts-title">"Keyboard shortcuts"</h2>
                <dl class="shortcut-list">{shortcuts.into_iter().map(|(label, keys)| view! {
                    <dt>{label}</dt><dd><kbd>{keys}</kbd></dd>
                }).collect_view()}</dl>
                <div class="dialog-actions"><button autofocus on:click=move |_|ui.studio.shortcuts.set(false)>"Close"</button></div>
            </StudioDialog>
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use browser::{ShortcutsModal, register};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_file_shortcuts_even_in_text_fields() {
        assert_eq!(shortcut("O", true, true, true), Some(Action::OpenProject));
        assert_eq!(shortcut("o", true, false, true), Some(Action::OpenVideo));
        assert_eq!(shortcut("S", true, true, true), Some(Action::SaveAs));
        assert_eq!(shortcut("s", true, false, true), Some(Action::Save));
    }

    #[test]
    fn protects_native_text_editing() {
        for key in [
            " ",
            "ArrowLeft",
            "ArrowRight",
            "i",
            "o",
            "Delete",
            "Backspace",
        ] {
            assert_eq!(shortcut(key, false, false, true), None);
        }
        assert_eq!(shortcut("z", true, false, true), None);
        assert_eq!(shortcut("Enter", true, false, true), None);
        assert_eq!(shortcut("F1", false, false, true), Some(Action::Help));
        assert_eq!(shortcut("Escape", false, false, true), Some(Action::Escape));
    }

    #[test]
    fn maps_transport_history_zoom_and_extraction() {
        assert_eq!(shortcut(" ", false, false, false), Some(Action::Play));
        assert_eq!(
            shortcut("ArrowLeft", false, false, false),
            Some(Action::Frame(-1))
        );
        assert_eq!(
            shortcut("ArrowRight", false, false, false),
            Some(Action::Frame(1))
        );
        assert_eq!(
            shortcut("ArrowRight", false, true, false),
            Some(Action::Step(1000))
        );
        assert_eq!(shortcut("z", true, false, false), Some(Action::Undo));
        assert_eq!(shortcut("Z", true, true, false), Some(Action::Redo));
        assert_eq!(shortcut("y", true, false, false), Some(Action::Redo));
        assert_eq!(shortcut("+", true, true, false), Some(Action::Zoom(1)));
        assert_eq!(shortcut("-", true, false, false), Some(Action::Zoom(-1)));
        assert_eq!(shortcut("0", true, false, false), Some(Action::Zoom(0)));
        assert_eq!(shortcut("Enter", true, false, false), Some(Action::Extract));
    }
}
