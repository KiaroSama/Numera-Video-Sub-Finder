# Models and native runtimes

`ci/models.json` pins the official PaddleOCR v5 detector and language recognizers by immutable Hub revision, exact expected bytes and SHA-256. Companion YAML dictionaries are embedded with provenance/hash metadata. Detector preprocessing uses BGR/NCHW normalized pixels; recognition uses aspect-preserving height 48, normalized-zero padding and ordered CTC decoding. Actual tensor vocabulary must match the dictionary; malformed/nonfinite outputs fail.

Persian visual token ordering is model-specific; stored/exported text remains logical Unicode. Optional Arabic Yeh/Kaf normalization is explicit and preserves raw OCR. Independent Tesseract `eng/fas/jpn` packs are pinned by `ci/tessdata.json`; scores from different engines are not compared as calibrated probabilities. Disagreement stays uncertain for review.

The application never downloads during inference. Explicit installation/import verifies identity before activation; missing/corrupt models are actionable errors. Models and libraries retain their upstream licenses. Byte integrity is separate from language/event quality.

Native ONNX Runtime is pinned to 1.31.0/API31, used by exact owner-approved `ort` 2.0.0-rc.14. Telemetry is disabled at environment initialization and source build. Intel macOS uses a source-built runtime where the official release inventory has no matching archive. Native FFmpeg/Tesseract are source-built from the pinned vcpkg baseline in CI; no Python application runtime is used.

Default public hosted tests are CPU. Optional CUDA/DirectML/CoreML feature code does not itself include or qualify a redistributable provider pack. Strict required-acceleration policies fail instead of silently running CPU; OS-managed CoreML placement cannot certify strict GPU-only execution.

Japanese specialist and local crop-only visual assistance require independently admitted model/dictionary/tokenizer/projector/native-runtime packs and bounded offline inference. They are not substituted by dummy text or a cloud endpoint. Until their admission/runtime/quality tests exist, these optional acceptance capabilities remain open.
