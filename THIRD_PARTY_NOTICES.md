# Third-party notices and provenance

Application-owned code is GPL-3.0-or-later; see LICENSE. Native dependencies, models and fixture media retain their own licenses. No license is inferred solely from a repository name or a model filename.

| Dependency/artifact | Pinned source | Declared license / obligations |
|---|---|---|
| Rust / Rust standard library | rust-toolchain.toml 1.99.0 | MIT / Apache-2.0; preserve notices |
| Tauri 2 / Leptos 0.8 | Cargo.lock and crate manifests | MIT / Apache-2.0 families; preserve exact package licenses |
| FFmpeg 9.0.2 | vcpkg baseline e456309491fd875487bf02b8a0dca76c1b1b00cc, vcpkg.json features | Applicable LGPL/GPL terms depend on exact features; nonfree disabled; preserve configuration and corresponding source |
| Tesseract 5.5.3 / Leptonica | Same vcpkg baseline | Apache-2.0 / BSD-style; preserve native notices |
| ONNX Runtime 1.31.0 | ci/targets.json official release hashes/source tag | MIT; preserve notices, runtime licenses and exact build provenance |
| ort 2.0.0-rc.14 | Cargo.lock | MIT / Apache-2.0; exact prerelease explicitly approved |
| webpki-root-certs 1.0.9 trust-anchor data | Cargo.lock; rustls/webpki-roots tag v/1.0.9 | CDLA-Permissive-2.0; full agreement in licenses/data/CDLA-Permissive-2.0.md is included with distributed data |
| PaddleOCR v5 ONNX models | ci/models.json immutable official PaddlePaddle Hub revisions | Declared Apache-2.0; hashes identify bytes, not benchmark quality |
| Tesseract language packs | ci/tessdata.json immutable tessdata_best revision | Apache-2.0 |
| Japanese v3 ONNX specialist | ci/models.json immutable conversion/base revisions | Declared Apache-2.0; separate quality/native admission |
| PaddleOCR-VL1.6 GGUF/projector | ci/assistant-models.json immutable official Hub revision | Declared Apache-2.0; opt-in candidate, no benchmark claim |
| llama.cpp native helper | stable v0.6.0 peeled commit d81235049384534c167caea52b85a694f6103d14 | MIT and preserved vendor notices; experimental CLI not release-qualified |
| tauri-plugin-webdriver 0.2.3 | Optional test-only upstream commit 50e7509073ca47dbf2f42a965d8161c198b96ac3, Windows COM compatibility correction | MIT; absent from production application profiles |
| Noto Sans / Arabic / Devanagari / KR fixture fonts | google/fonts commit 1d0e46c7933245c8180fe83e47917961c913bbdb; exact bytes in xtask/src/multilingual.rs | SIL Open Font License 1.1; unmodified font bytes and original per-font copyright/license retained in hosted fixture resources, not application packages |
| Official project logo | Supplied by project owner | Owner-authorized project branding; no unrelated proprietary logo copied |

The native build preserves vcpkg copyright files and ONNX release/source LICENSE/notices with the package. The Rust lockfile and native/model manifests identify inputs. A release must ship an actual dependency/license inventory and provide corresponding source/configuration required by the selected licenses; this document is not a substitute for collecting exact package notices.

Real fixture media, where acquired, must carry its separate source/author/license attribution, original hash, derivative changes and any ShareAlike requirements. A dataset annotation license does not grant rights to an underlying video. Synthetic generated fixtures are clearly labelled and never advertised as real hardsub quality evidence.

This project independently implements its Rust pipeline; no source from unrelated VideoSubFinder applications was copied. Framework/native dependency APIs and official documentation were researched, not treated as permission to copy incompatible licensed code.
