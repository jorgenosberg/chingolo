# third_party

`oar-ocr` and `oar-ocr-core` 0.10.2 (Apache-2.0, https://github.com/GreatV/oar-ocr), source only.

Changed from upstream: `ort` is `=2.0.0-rc.15` instead of `=2.0.0-rc.13`, so it links with the one ONNX Runtime that `fastembed` uses.
The ort environment code in `oar-ocr-core/src/core/inference/{environment,session,ort_infer_config}.rs` is adapted to the rc.15 API. Recognized text is byte-identical to the unpatched crate on our test pages.
Drop this folder and the `[patch.crates-io]` entry once upstream moves to rc.15.
