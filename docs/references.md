# Primary references and provenance

Reviewed 2026-10-13. These sources establish API/provenance decisions, not product test results.

- [Tauri 2 resource resolution](https://v2.tauri.app/develop/resources/) and [configuration](https://v2.tauri.app/reference/config/): native GUI/resource/sidecar packaging, scoped commands and generated glue.
- [Leptos](https://book.leptos.dev/): Rust CSR frontend and reactive components.
- [FFmpeg](https://ffmpeg.org/documentation.html), [ffmpeg-next 9](https://docs.rs/ffmpeg-next/9.0.0/ffmpeg_next/) and [pinned vcpkg source](https://github.com/microsoft/vcpkg/tree/e456309491fd875487bf02b8a0dca76c1b1b00cc): source PTS/decode ownership and target-native builds.
- [ort](https://github.com/pykeio/ort) and [ONNX Runtime 1.31](https://github.com/microsoft/onnxruntime/releases/tag/v1.31.0): explicit environment/session/provider contracts. Exact ort RC is an intentional approved pin, not a stable release claim.
- [Tesseract](https://github.com/tesseract-ocr/tesseract) and [tessdata](https://github.com/tesseract-ocr/tessdata): native verifier/language-pack provenance.
- [PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR), [official Paddle model organization](https://huggingface.co/PaddlePaddle) and [llama.cpp](https://github.com/ggml-org/llama.cpp): model metadata/CTC and optional local visual retry. Manifest immutable revisions/hashes are authoritative for artifact identity.
- [cargo-llvm-cov 0.9.1](https://github.com/taiki-e/cargo-llvm-cov/tree/v0.9.1), [cargo-mutants 27.1](https://github.com/sourcefrog/cargo-mutants/tree/v27.1.0), [Miri](https://github.com/rust-lang/miri): bounded source-coverage/assertion-strength/runtime analysis.
- [RustSec](https://github.com/rustsec/rustsec), [cargo-deny 0.20.2](https://github.com/EmbarkStudios/cargo-deny/tree/0.20.2), [CodeQL](https://docs.github.com/en/code-security/code-scanning/introduction-to-code-scanning/about-code-scanning-with-codeql): dependency/license/source checks, never exhaustive security assurance.

Real fixture rights and candidate URLs are in [fixture manifest](../fixtures/manifest.json). Independently annotated visible text/source-frame timing is still required; automatic OCR output is not ground truth. Third-party code/native/model/media rights remain separate in [notices](../THIRD_PARTY_NOTICES.md).
