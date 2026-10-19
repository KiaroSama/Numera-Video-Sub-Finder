use nvsf_app::{Paths, updates};
use nvsf_core::maintenance::Preferences;

#[test]
fn release_metadata_rejects_untrusted_targets_and_denied_channels() {
    let manifest = serde_json::json!({
        "version":"0.2.0", "notes":"New release",
        "platforms":{"linux-x86_64-appimage":{
            "url":"https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/download/v0.2.0/app.AppImage",
            "signature":"test-public-signature", "sha256":"a".repeat(64),"bytes":123
        }}
    });
    let encode = |v: &serde_json::Value| serde_json::to_vec(v).unwrap();
    assert!(
        updates::manifest(&encode(&manifest), "0.1.0", "linux-x86_64-appimage")
            .unwrap()
            .is_some()
    );
    assert!(
        updates::manifest(&encode(&manifest), "0.2.0", "linux-x86_64-appimage")
            .unwrap()
            .is_none()
    );
    assert!(updates::manifest(&encode(&manifest), "0.1.0", "windows-aarch64-nsis").is_err());
    let mut untrusted = manifest.clone();
    untrusted["platforms"]["linux-x86_64-appimage"]["url"] =
        "https://example.com/app.AppImage".into();
    assert!(updates::manifest(&encode(&untrusted), "0.1.0", "linux-x86_64-appimage").is_err());
    untrusted = manifest.clone();
    untrusted["version"] = "0.2.0-alpha.1".into();
    assert!(
        updates::manifest(&encode(&untrusted), "0.1.0", "linux-x86_64-appimage")
            .unwrap()
            .is_none()
    );
    for url in [
        "http://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/download/v1/app",
        "https://github.com/another/project/releases/download/v1/app",
        "https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/download/v1/%2e%2e/app",
        "https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/download/v1/app?token=not-allowed",
        "https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/download/v1/",
        "https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/download/v1/subdir/app",
    ] {
        assert!(updates::official_asset(url).is_err());
    }
}

#[test]
fn signed_metadata_and_payloads_refuse_invalid_signatures_formats_and_architectures() {
    use nvsf_app::update_payload::validate;
    assert!(updates::verify_signature(b"{}", b"invalid signature").is_err());
    let directory = tempfile::tempdir().unwrap();
    let artifact = directory.path().join("update.bin");
    std::fs::write(&artifact, b"unsigned payload").unwrap();
    assert!(updates::verify_artifact_file(&artifact, b"invalid signature", "0.2.0").is_err());
    assert_eq!(std::fs::read(&artifact).unwrap(), b"unsigned payload");
    assert!(validate(b"not an ELF", "linux-x86_64-appimage").is_err());
    let mut elf = vec![0; 64];
    elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
    elf[8..11].copy_from_slice(b"AI\x02");
    elf[16..18].copy_from_slice(&3u16.to_le_bytes());
    elf[18..20].copy_from_slice(&62u16.to_le_bytes());
    assert!(validate(&elf, "linux-x86_64-appimage").is_ok());
    assert!(validate(&elf, "linux-aarch64-appimage").is_err());
    assert!(validate(&elf, "windows-x86_64-nsis").is_err());
    assert!(validate(&elf, "darwin-aarch64-app").is_err());
    assert!(validate(&elf, "linux-x86_64-unknown").is_err());
    let mut pe = vec![0; 70];
    pe[..2].copy_from_slice(b"MZ");
    pe[60..64].copy_from_slice(&64u32.to_le_bytes());
    pe[64..68].copy_from_slice(b"PE\0\0");
    pe[68..70].copy_from_slice(&0x14cu16.to_le_bytes());
    assert!(validate(&pe, "windows-x86_64-nsis").is_ok());
    assert!(
        validate(&pe, "windows-aarch64-nsis").is_ok(),
        "NSIS stub is x86; signed inventory binds application architecture"
    );
    pe[60..64].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(validate(&pe, "windows-x86_64-nsis").is_err());
}

#[test]
fn macos_update_archive_checks_real_headers_and_forbids_wrong_target_or_links() {
    use std::io::Write;
    let archive = |wrong_arch: bool, link: bool| {
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut tar = tar::Builder::new(&mut gzip);
            for name in ["numera-video-sub-finder", "nvsf"] {
                let mut executable = [0u8; 32];
                executable[..4].copy_from_slice(b"\xcf\xfa\xed\xfe");
                executable[4..8].copy_from_slice(
                    &(if wrong_arch {
                        0x0100000cu32
                    } else {
                        0x01000007u32
                    })
                    .to_le_bytes(),
                );
                executable[12..16].copy_from_slice(&2u32.to_le_bytes());
                let mut header = tar::Header::new_gnu();
                header.set_size(executable.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                tar.append_data(
                    &mut header,
                    format!("Numera.app/Contents/MacOS/{name}"),
                    executable.as_slice(),
                )
                .unwrap();
            }
            if link {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                header.set_mode(0o777);
                tar.append_link(&mut header, "Numera.app/Contents/link", "../../outside")
                    .unwrap();
            }
            tar.finish().unwrap();
        }
        gzip.flush().unwrap();
        gzip.finish().unwrap()
    };
    let valid = archive(false, false);
    assert!(nvsf_app::update_payload::validate(&valid, "darwin-x86_64-app").is_ok());
    assert!(
        nvsf_app::update_payload::validate(&archive(true, false), "darwin-x86_64-app").is_err()
    );
    assert!(
        nvsf_app::update_payload::validate(&archive(false, true), "darwin-x86_64-app").is_err()
    );
    assert!(
        nvsf_app::update_payload::validate(&valid[..valid.len() - 8], "darwin-x86_64-app").is_err()
    );
    let mut trailing = valid.clone();
    trailing.extend_from_slice(b"trailing");
    assert!(nvsf_app::update_payload::validate(&trailing, "darwin-x86_64-app").is_err());
}

#[test]
fn update_contract_limits_reject_malformed_metadata_without_mutating_preferences() {
    let artifact = updates::Artifact {
        url:
            "https://github.com/KiaroSama/Numera-Video-Sub-Finder/releases/download/v0.2.0/app.bin"
                .into(),
        signature: "fixture-not-an-authenticated-signature".into(),
        sha256: "a".repeat(64),
        bytes: 1,
    };
    for bytes in [0, updates::MAX_UPDATE_BYTES + 1] {
        let mut invalid = artifact.clone();
        invalid.bytes = bytes;
        assert!(updates::validate_artifact(&invalid).is_err());
    }
    for hash in ["a".repeat(63), "z".repeat(64)] {
        let mut invalid = artifact.clone();
        invalid.sha256 = hash;
        assert!(updates::validate_artifact(&invalid).is_err());
    }
    for signature in [String::new(), "s".repeat(4097)] {
        let mut invalid = artifact.clone();
        invalid.signature = signature;
        assert!(updates::validate_artifact(&invalid).is_err());
    }
    let mut manifest = serde_json::json!({
        "version":"0.2.0","notes":"n".repeat(16385),
        "platforms":{"fixture":artifact}
    });
    assert!(
        updates::manifest(&serde_json::to_vec(&manifest).unwrap(), "0.1.0", "fixture").is_err()
    );
    manifest["notes"] = "".into();
    manifest["platforms"]["fixture"]["unknown"] = true.into();
    assert!(
        updates::manifest(&serde_json::to_vec(&manifest).unwrap(), "0.1.0", "fixture").is_err()
    );
    assert_eq!(
        updates::manifest(&vec![b' '; 1024 * 1024 + 1], "0.1.0", "fixture")
            .unwrap_err()
            .to_string(),
        "Operation failed: Update metadata exceeds 1MiB"
    );
    assert_eq!(
        updates::verify_signature(&vec![0; 1024 * 1024 + 1], b"invalid")
            .unwrap_err()
            .to_string(),
        "Operation failed: Signed metadata exceeds limits"
    );
    assert_eq!(
        updates::verify_signature(b"{}", &vec![0; 4097])
            .unwrap_err()
            .to_string(),
        "Operation failed: Release signature exceeds limits"
    );

    let directory = tempfile::tempdir().unwrap();
    let paths = Paths::new(Some(directory.path())).unwrap();
    let path = paths.root.join("updates.json");
    for value in [
        b"{\"schema\":2,\"enabled\":false,\"last_attempt\":null}".as_slice(),
        b"{\"schema\":1,\"enabled\":false,\"last_attempt\":null,\"unknown\":true}".as_slice(),
        b"not json".as_slice(),
    ] {
        std::fs::write(&path, value).unwrap();
        assert!(updates::preferences(&paths).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), value);
    }
}

#[test]
fn update_preferences_are_atomic_bounded_and_preserve_disabled_choice() {
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths::new(Some(directory.path())).unwrap();
    assert!(updates::preferences(&paths).unwrap().enabled);
    let preferences = Preferences {
        enabled: false,
        last_attempt: Some(123),
        ..Default::default()
    };
    updates::save_preferences(&paths, &preferences).unwrap();
    let loaded = updates::preferences(&paths).unwrap();
    assert!(!loaded.enabled);
    assert_eq!(loaded.last_attempt, Some(123));
    std::fs::write(paths.root.join("updates.json"), vec![b' '; 4097]).unwrap();
    assert!(updates::preferences(&paths).is_err());
}
