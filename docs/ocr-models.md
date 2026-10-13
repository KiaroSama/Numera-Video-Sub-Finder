# OCR model contracts

[Model manifests](../ci/models.json) pin immutable revision, byte count, SHA-256, license and matching YAML/dictionary hash. [Tesseract language packs](../ci/tessdata.json) independently pin eng/fas/jpn. Never activate an unknown model or update it during extraction.

The baseline is PP-OCRv5 mobile detection plus the exact English/Japanese or Arabic/Persian recognition model. Detection uses BGR/NCHW, the declared scale/mean/std and a 960-pixel bounded resize. Recognition preserves aspect ratio at height 48, BGR [-1,1] and normalized-zero padding; CTC uses exact dictionary order, blank removal and adjacent-repeat collapse. Shape/vocabulary/finite-value mismatches fail. Current detection postprocessing is an axis-aligned connected-region approximation: rotated polygon/unclip parity and real-language quality are not yet qualified.

English/Persian/Japanese route explicitly. Persian correction is model-specific visual token ordering; stored text remains logical Unicode, with optional explicit Yeh/Kaf normalization. Tesseract and ONNX scores are not interchangeable probabilities; disagreement remains a review candidate rather than a numeric cross-engine winner.

The optional Japanese v3 ONNX specialist is a community conversion with official base provenance, separate checksum and metadata. It is installed explicitly; no superiority/parity claim is made without a measured benchmark. Local visual retry uses a separately admitted PaddleOCR-VL GGUF/projector pair, about 1.82 GB of file bytes, and packaged native llama.cpp. It produces review-only proposals or unreadable status, never accepted times or automatic dialogue.

Download supports verified partial resumption, complete-partial offline activation and cancellation between chunks. A blocking network read may defer cancellation until its timeout. Current import/export accepts single checksummed ONNX files, not arbitrary archives; external-data ONNX/archive packs are not supported and must not be represented as admitted. See [models](models.md) for operations and [validation](validation.md) for pending native/quality evidence.
