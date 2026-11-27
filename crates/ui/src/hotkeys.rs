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
    FocusMode,
    ZoomTool,
    Stop,
    Speed(i8),
    SeekStart,
    SeekEnd,
    Guides,
    ExclusionMode,
    ToggleTheme,
    AddQueue,
    Route(usize),
    Notifications,
    Help,
    Escape,
}

fn shortcut_with_alt(
    key: &str,
    control: bool,
    shift: bool,
    alt: bool,
    editing: bool,
) -> Option<Action> {
    if alt {
        return (!control && !shift && matches!(key, "n" | "N")).then_some(Action::Notifications);
    }
    shortcut(key, control, shift, editing)
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
        ("t" | "T", true, true) => Some(ToggleTheme),
        ("q" | "Q", true, false) => Some(AddQueue),
        ("1" | "2" | "3" | "4" | "5" | "6", true, false) => {
            key.parse::<usize>().ok().map(|digit| Route(digit - 1))
        }
        _ if editing => None,
        ("`" | "Backquote", false, false) => Some(FocusMode),
        ("z" | "Z", false, false) => Some(ZoomTool),
        ("s" | "S", false, false) => Some(Stop),
        ("j" | "J", false, false) => Some(Step(-10_000)),
        ("k" | "K", false, false) => Some(Play),
        ("l" | "L", false, false) => Some(Step(10_000)),
        ("[", false, false) => Some(Speed(-1)),
        ("]", false, false) => Some(Speed(1)),
        ("Home", false, false) => Some(SeekStart),
        ("End", false, false) => Some(SeekEnd),
        ("g" | "G", false, false) => Some(Guides),
        ("e" | "E", false, false) => Some(ExclusionMode),
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

#[derive(Debug, PartialEq, Eq)]
enum ModalKey {
    Run,
    Native,
    Block,
}

fn modal_key(action: Action, open: bool, shortcuts_target: bool) -> ModalKey {
    match action {
        _ if !open => ModalKey::Run,
        Action::Help | Action::Escape if shortcuts_target => ModalKey::Run,
        Action::Escape => ModalKey::Native,
        _ => ModalKey::Block,
    }
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use super::{Action, ModalKey, modal_key, shortcut_with_alt};
    use crate::{close_modal::StudioDialog, state::Ui};
    use leptos::{ev, prelude::*};
    use wasm_bindgen::JsCast;

    pub(crate) fn register(ui: Ui) {
        let handle = window_event_listener(ev::keydown, move |event| {
            if event.default_prevented() || event.is_composing() || event.key_code() == 229 {
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
            let control = event.ctrl_key() || event.meta_key();
            let shift = event.shift_key();
            let alt = event.alt_key();
            let dialog = document().query_selector("dialog[open]").ok().flatten();
            let closing = ui.studio.close_open.get_untracked()
                || ui.closing.get_untracked()
                || web_sys::window().is_some_and(|w| {
                    js_sys::Reflect::has(&w, &"__NVSF_CLOSE_LOCK__".into()).unwrap_or(true)
                });
            if closing || dialog.is_some() {
                // Preserve native typing/activation; block workspace shortcuts in modal dialogs.
                if let Some(action) =
                    shortcut_with_alt(&key, control, shift, alt, editing || activate)
                {
                    let shortcuts_target = !closing
                        && target.as_ref().is_some_and(|target| {
                            target
                                .closest("#shortcuts-modal[open]")
                                .ok()
                                .flatten()
                                .is_some()
                        });
                    match modal_key(action, true, shortcuts_target) {
                        ModalKey::Run => {}
                        ModalKey::Native => return,
                        ModalKey::Block => {
                            event.prevent_default();
                            return;
                        }
                    }
                } else {
                    return;
                }
            }
            let Some(action) = shortcut_with_alt(&key, control, shift, alt, editing || activate)
            else {
                return;
            };
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
                Action::Stop if preview => ui.stop_playback(),
                Action::Speed(delta) if preview => ui.adjust_playback_rate(delta),
                Action::SeekStart if preview => ui.seek_start(),
                Action::SeekEnd if preview => ui.seek_end(),
                Action::Step(delta) if preview => ui.step(delta),
                Action::Frame(direction) if preview => ui.step_frame(direction),
                Action::SetStart if extract => ui.mark_range(false),
                Action::SetEnd if extract => ui.mark_range(true),
                Action::DeleteRegion if extract && ui.remove_selected_region() => {}
                Action::Zoom(delta) if preview => ui.change_zoom(delta),
                Action::Extract if extract && ui.can_extract() => ui.start_extraction(),
                Action::FocusMode => ui.toggle_focus_mode(),
                Action::ZoomTool if preview => ui.toggle_zoom_tool(),
                Action::Guides if extract => ui.toggle_guides(),
                Action::ExclusionMode if extract => ui.toggle_exclusion_mode(),
                Action::ToggleTheme => ui.toggle_theme(),
                Action::AddQueue => ui.add_queue(),
                Action::Route(index) => {
                    if let Some(route) = crate::helpers::ROUTES.get(index) {
                        ui.route.set((*route).into());
                    }
                }
                Action::Notifications if !ui.studio.focus_mode.get_untracked() => {
                    ui.toggle_notifications();
                }
                Action::Help => ui.studio.shortcuts.update(|open| *open = !*open),
                Action::Escape if ui.studio.shortcuts.get_untracked() => {
                    ui.studio.shortcuts.set(false)
                }
                Action::Escape if ui.notifications_open.get_untracked() => {
                    ui.close_notifications(true);
                }
                Action::Escape if ui.studio.menu_open.get_untracked().is_some() => {
                    crate::menubar::dismiss_menu(ui);
                }
                Action::Escape if ui.studio.focus_mode.get_untracked() => ui.toggle_focus_mode(),
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
        let shortcuts: [(&str, &[(&str, &str)]); 3] = [
            (
                "Workspace",
                &[
                    ("Open video / project", "Ctrl+O / Ctrl+Shift+O"),
                    ("Save / Save As", "Ctrl+S / Ctrl+Shift+S"),
                    ("Undo / Redo", "Ctrl+Z / Ctrl+Y or Ctrl+Shift+Z"),
                    ("Add to queue", "Ctrl+Q"),
                    (
                        "Extract, Queue, Review, Models, History, Settings",
                        "Ctrl+1 to Ctrl+6",
                    ),
                    ("Switch theme", "Ctrl+Shift+T"),
                    ("Notifications", "Alt+N"),
                    ("Keyboard shortcuts", "F1"),
                    ("Close overlay, exit Focus mode, or cancel drawing", "Esc"),
                    ("Move focus", "Tab / Shift+Tab"),
                ],
            ),
            (
                "Playback",
                &[
                    ("Play / Pause", "Space / K"),
                    ("Stop", "S"),
                    ("Previous / next frame", "← / →"),
                    ("Jump back / forward one second", "Shift+← / Shift+→"),
                    ("Jump back / forward ten seconds", "J / L"),
                    ("Decrease / increase speed", "[ / ]"),
                    ("Seek to start / end", "Home / End"),
                ],
            ),
            (
                "Video and extraction",
                &[
                    ("Focus mode", "`"),
                    ("Zoom tool", "Z"),
                    ("Zoom in / out / Fit", "Ctrl+= / Ctrl+- / Ctrl+0"),
                    ("Guides", "G"),
                    ("Exclusion mode", "E"),
                    ("Set range start / end", "I / O"),
                    ("Delete selected region", "Delete / Backspace"),
                    ("Extract subtitles", "Ctrl+Enter"),
                ],
            ),
        ];
        view! {
            <StudioDialog id="shortcuts-modal" label="shortcuts-title" role="dialog"
                visible=ui.studio.shortcuts on_cancel=move ||ui.studio.shortcuts.set(false)>
                <h2 id="shortcuts-title">"Keyboard shortcuts"</h2>
                {shortcuts.into_iter().map(|(group, entries)| view! {
                    <h3>{group}</h3>
                    <dl class="shortcut-list">{entries.iter().copied().map(|(label, keys)| view! {
                        <dt>{label}</dt><dd><kbd>{keys}</kbd></dd>
                    }).collect_view()}</dl>
                }).collect_view()}
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
    fn maps_studio_keys_without_stealing_text_editing() {
        for (key, action) in [
            ("`", Action::FocusMode),
            ("Backquote", Action::FocusMode),
            ("z", Action::ZoomTool),
            ("s", Action::Stop),
            ("j", Action::Step(-10_000)),
            ("k", Action::Play),
            ("l", Action::Step(10_000)),
            ("[", Action::Speed(-1)),
            ("]", Action::Speed(1)),
            ("Home", Action::SeekStart),
            ("End", Action::SeekEnd),
            ("g", Action::Guides),
            ("e", Action::ExclusionMode),
        ] {
            assert_eq!(shortcut(key, false, false, false), Some(action));
            assert_eq!(shortcut(key, false, false, true), None);
            assert_eq!(shortcut_with_alt(key, false, false, true, false), None);
        }
    }

    #[test]
    fn maps_workspace_shortcuts() {
        assert_eq!(shortcut("T", true, true, true), Some(Action::ToggleTheme));
        assert_eq!(shortcut("q", true, false, true), Some(Action::AddQueue));
        for (index, key) in ["1", "2", "3", "4", "5", "6"].into_iter().enumerate() {
            assert_eq!(
                shortcut(key, true, false, false),
                Some(Action::Route(index))
            );
            assert_eq!(shortcut(key, false, false, false), None);
        }
        assert_eq!(
            shortcut_with_alt("n", false, false, true, false),
            Some(Action::Notifications)
        );
        assert_eq!(shortcut_with_alt("n", true, false, true, false), None);
        assert_eq!(shortcut("n", false, false, false), None);
    }

    #[test]
    fn isolates_all_mapped_shortcuts_in_modal_dialogs() {
        for action in [
            Action::OpenVideo,
            Action::Undo,
            Action::Play,
            Action::FocusMode,
            Action::Route(0),
            Action::Notifications,
        ] {
            assert_eq!(modal_key(action, true, true), ModalKey::Block);
            assert_eq!(modal_key(action, true, false), ModalKey::Block);
            assert_eq!(modal_key(action, false, false), ModalKey::Run);
        }
        assert_eq!(modal_key(Action::Help, true, false), ModalKey::Block);
        assert_eq!(modal_key(Action::Escape, true, false), ModalKey::Native);
        assert_eq!(modal_key(Action::Help, true, true), ModalKey::Run);
        assert_eq!(modal_key(Action::Escape, true, true), ModalKey::Run);
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
