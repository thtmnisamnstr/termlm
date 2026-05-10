# termlm Quick Recipes

Use these as practical prompt examples.

## Find references in a codebase

Prompt:

`? find where the Redis client is initialized`

Expected behavior:

- local file/search tools are used
- proposal likely includes `rg` command(s)

## Fix failing tests quickly

Prompt:

`? run the test suite and summarize first failure`

Expected behavior:

- command proposal to run tests
- result captured and summarized

## Reindex after PATH tool changes

Prompt:

`? refresh command docs after installing new CLI tools`

Expected behavior:

- proposes `termlm reindex --mode delta`

## Gather git context

Prompt:

`? summarize what changed in this branch`

Expected behavior:

- uses git context local tool
- may propose read-only git commands

## Web-backed freshness question

Prompt:

`? what is the latest ollama tool-calling API behavior? include sources`

Expected behavior:

- web tools are exposed for freshness-required query
- response cites sources

## Session workflow

1. Enter session mode: `/p`
2. Ask iterative prompts without exiting
3. Exit session mode: `/q`
