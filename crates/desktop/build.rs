fn main() {
    println!("cargo:rerun-if-changed=capabilities-e2e");
    let attributes = tauri_build::Attributes::new().windows_attributes(
        tauri_build::WindowsAttributes::new().window_icon_path("icons/icon.ico"),
    );
    #[cfg(feature = "e2e")]
    let attributes = attributes.capabilities_path_pattern("capabilities-e2e/*.json");
    tauri_build::try_build(
        attributes.app_manifest(tauri_build::AppManifest::new().commands(&[
            "ui_ready",
            "set_unsaved",
            "update_status",
            "check_updates",
            "set_update_checks",
            "download_update",
            "install_update",
            "cancel_update",
            "save_update",
            "pick_source",
            "choose_output",
            "preview_frame",
            "start_job",
            "queue_list",
            "queue_add",
            "queue_remove",
            "queue_regrant",
            "queue_start",
            "pick_checkpoint",
            "job_status",
            "control_job",
            "current_project",
            "replace_project",
            "save_project",
            "export_project",
            "pick_project",
            "list_history",
            "open_history",
            "model_catalog",
            "language_catalog",
            "install_language",
            "import_language",
            "export_language",
            "install_models",
            "cancel_model_install",
            "import_model",
            "export_model",
            "doctor",
            "repair_runtimes",
            "suggest_regions",
            "recognize_cue",
            "install_visual_retry",
            "cancel_visual_retry",
            "propose_visual_retry",
            #[cfg(feature = "e2e")]
            "e2e_fixture",
            #[cfg(feature = "e2e")]
            "e2e_save_edits",
            #[cfg(feature = "e2e")]
            "e2e_focus",
        ])),
    )
    .expect("Native desktop resources and command permissions must be valid");
}
