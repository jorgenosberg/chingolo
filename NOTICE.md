# Third-party notices

Chingolo is MIT licensed (see `LICENSE`). It includes or downloads the following. The app bundle carries a copy of these notices in `Contents/Resources/licenses`.

## Included in the app

- **PDFium**, built by [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) (MIT, see `licenses/pdfium/LICENSE`). The PDFium build links other libraries (FreeType, HarfBuzz, ICU, LittleCMS, Abseil and others). Their licenses are in `licenses/pdfium/third-party/`.
- **oar-ocr** 0.10.2 (Apache-2.0), vendored in `src-tauri/third_party` with a small change, see its README. It runs the text recognition, page layout and table models.
- **Rust and JavaScript dependencies.** See `src-tauri/Cargo.lock` and `bun.lock` for the full list. Direct dependencies include Tauri, fastembed-rs and ONNX Runtime through the `ort` crate (all Apache-2.0 or MIT).

## Downloaded on first use

These are not part of the app download. The app fetches them when you turn the feature on, and checks their SHA-256.

- **PP-DocLayoutV3** (page layout), **SLANet+** (table structure) and **PP-OCRv6** (text recognition on scanned pages). ONNX versions of PaddlePaddle models, published with the oar-ocr releases at <https://github.com/GreatV/oar-ocr/releases> (Apache-2.0).
- **EmbeddingGemma 300M**, optional local search model, ONNX version from <https://huggingface.co/onnx-community/embeddinggemma-300m-ONNX>. It is licensed under the [Gemma Terms of Use](https://ai.google.dev/gemma/terms), which the user accepts by downloading it. The default search model is Gemini, through the user's own key.

## Services

Chingolo sends text to the AI provider the user chooses (Gemini or OpenAI) with the user's own API key. To fill in a paper's details it sends that paper's DOI, title or ISBN to Crossref, DataCite, doi.org, PubMed, Open Library and the National Library of Norway. It reads a public price list from models.dev, and checks GitHub for new versions. Papers themselves, the library database and the API keys stay on the Mac.
