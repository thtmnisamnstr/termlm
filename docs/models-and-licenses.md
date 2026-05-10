# Models and Licenses

`termlm` release artifacts can include model assets. Ensure your usage complies with each model's
license and terms.

## Default model assets in with-models releases

- `gemma-4-E4B-it-Q4_K_M.gguf`
- `bge-small-en-v1.5.Q4_K_M.gguf`

Optional release-time inclusion:

- `gemma-4-E2B-it-Q4_K_M.gguf`

## Upstream sources

- Gemma GGUF variants: [ggml-org/gemma-4-E4B-it-GGUF](https://huggingface.co/ggml-org/gemma-4-E4B-it-GGUF)
- Gemma E2B GGUF variant: [ggml-org/gemma-4-E2B-it-GGUF](https://huggingface.co/ggml-org/gemma-4-E2B-it-GGUF)
- BGE embedding GGUF: [ChristianAzinn/bge-small-en-v1.5-gguf](https://huggingface.co/ChristianAzinn/bge-small-en-v1.5-gguf)

## Notes

- `termlm upgrade` uses `no-models` artifacts, preserves existing inference models, and bootstraps
  embeddings/index assets if missing.
- Verify model provenance/checksums in your deployment process.
- Some models have usage restrictions beyond OSS licensing; review upstream terms before distribution.
