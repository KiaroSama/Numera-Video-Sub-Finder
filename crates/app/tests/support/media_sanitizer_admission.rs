use nvsf_app::media::NATIVE_LIBRARY_NAME;
use nvsf_app::storage::hash_file;
use std::path::{Path, PathBuf};

pub fn selected_root() -> Option<PathBuf> {
    let root = std::env::var_os("NVSF_MEDIA_SANITIZER_ROOT")?;
    assert!(cfg!(all(target_os = "linux", target_arch = "x86_64")));
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(
        std::env::var("RUNNER_ENVIRONMENT").as_deref(),
        Ok("github-hosted")
    );
    assert_eq!(
        std::env::var("NVSF_MEDIA_SANITIZER").as_deref(),
        Ok("address")
    );
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let expected = workspace.join(".ci-work/media-sanitizer/runtimes");
    let root = PathBuf::from(root);
    assert!(root.is_absolute());
    assert_eq!(root, expected);
    assert_eq!(root.canonicalize().unwrap(), expected);
    Some(root)
}

fn hash(path: &Path) -> String {
    hash_file(path).unwrap()
}

// Called while the actual MediaRuntime still owns its loaded library, in each real Y4M test.
pub fn verify_loaded(root: &Path) {
    if selected_root().is_none() {
        return;
    }
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.parent().unwrap().join("staged-libraries.json")).unwrap(),
    )
    .unwrap();
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    let mapped: std::collections::BTreeSet<_> = maps
        .lines()
        .filter_map(|line| line.split_whitespace().nth(5))
        .filter(|path| path.starts_with('/'))
        .map(PathBuf::from)
        .collect();
    let media: Vec<_> = mapped
        .iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name == NATIVE_LIBRARY_NAME)
        })
        .collect();
    assert_eq!(media, [&root.join(NATIVE_LIBRARY_NAME)]);
    let mut observed = Vec::new();
    for base in [
        "libnvsf_media_native.so",
        "libavcodec.so",
        "libavformat.so",
        "libavutil.so",
        "libswscale.so",
    ] {
        let files: Vec<_> = mapped
            .iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == base || name.starts_with(&format!("{base}.")))
            })
            .collect();
        assert!(!files.is_empty(), "required library not mapped: {base}");
        for file in files {
            assert_eq!(
                file.parent(),
                Some(root),
                "unexpected loaded closure: {file:?}"
            );
            let name = file.file_name().unwrap().to_str().unwrap();
            let digest = hash(file);
            assert_eq!(manifest[name].as_str(), Some(digest.as_str()));
            observed.push(serde_json::json!({"path":file,"sha256":digest}));
        }
    }
    println!(
        "NVSF_MEDIA_ASAN_LOADED={}",
        serde_json::json!({"libraries":observed,"adapter_sha256":hash(&root.join(NATIVE_LIBRARY_NAME))})
    );
}
