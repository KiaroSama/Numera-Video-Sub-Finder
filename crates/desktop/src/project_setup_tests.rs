use super::*;

#[test]
fn extract_setup_saves_and_loads_without_cues() {
    let (directory, app) = workspace();
    let host = app.state::<Host>();
    let source = directory.path().join("source.mp4");
    std::fs::write(&source, b"source video").unwrap();
    lock(&host.inner().0.sources)
        .unwrap()
        .insert("source-token".into(), source.canonicalize().unwrap());
    let mut setup = project();
    setup.source_hash = nvsf_app::storage::hash_file(&source).unwrap();
    setup.config.input = "source-token".into();
    setup.config.output.clear();
    setup.config.language = "fa".into();
    setup.config.device = "cpu".into();
    setup.config.quality = "fast".into();
    setup.config.start_ms = 500;
    setup.config.end_ms = Some(5_000);
    setup.config.exclusions.push(Rect {
        x: 0.1,
        y: 0.1,
        width: 0.2,
        height: 0.2,
    });
    setup.cues.clear();
    setup.model_hash.clear();
    setup.processed_ms = 0;
    tauri::async_runtime::block_on(files::replace_project(
        host.clone(),
        app.state(),
        setup,
        Some("source-token".into()),
    ))
    .unwrap();
    let active = lock(&host.inner().0.project).unwrap().clone().unwrap();
    assert_eq!(
        std::path::Path::new(&active.config.input),
        source.canonicalize().unwrap()
    );
    assert_ne!(
        files::current_project(host).unwrap().unwrap().config.input,
        active.config.input
    );
    let expected = serde_json::to_value(&active.config).unwrap();
    assert_eq!(files::project_filename("clip.nvsf.json"), "clip.nvsf");
    assert_eq!(files::project_filename("clip.nvsf"), "clip.nvsf");
    assert_eq!(files::project_filename("clip.json"), "clip.nvsf");
    for extension in ["nvsf", "nvsf.json", "json"] {
        let path = directory.path().join(format!("clip.{extension}"));
        files::save_project_to(&active, &path, || Ok(true)).unwrap();
        let loaded = nvsf_app::storage::load_project(&path).unwrap();
        assert!(loaded.cues.is_empty());
        assert_eq!(serde_json::to_value(loaded.config).unwrap(), expected);
    }
}
