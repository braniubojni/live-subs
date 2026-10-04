# 04 Cleanup

## Goal
Remove fillers, fix punctuation and case on a final line within a hard 700 ms budget. Never delay or corrupt the subtitle. On any doubt, show the raw line.

## Current state
[src/cleanup.rs](../../src/cleanup.rs) has two `Cleaner` implementations sharing one ChatML prompt and one output tail (first non-empty line, then `accept_cleanup`, which rejects empty or more than 2x length, unchanged).

- `LlamaCli`: spawns `llama-completion -no-cnv` per line, `-n 64 --temp 0 -c 512 -ngl 99`, `kill_on_drop(true)`, 700 ms timeout. It reloads the model every call. A timeout returns an error that the pipeline turns into the raw line; a non-zero exit returns the raw line directly. The model path is passed as an `OsStr`.
- `LlamaServer`: `LlamaServer::spawn` starts `llama-server` once on a free `127.0.0.1` port (`-c 512 -ngl 99`, `kill_on_drop(true)`) and keeps it warm. Each line is a `POST /completion` with `n_predict: 64`, `temperature: 0`, `cache_prompt: true` and the same system prompt, under the same 700 ms timeout. Any connect, HTTP status, parse or timeout failure returns the raw line. There is no readiness wait: while the model loads the server answers 503, which also returns raw. The HTTP client is hand-written HTTP/1.0 over `tokio::net::TcpStream`, so there is no new dependency.

`main` uses `LlamaServer` when `LLAMA_SERVER` (path to `llama-server`) is set, otherwise `LlamaCli`. Nothing restarts a dead server yet (see Health and restart). The p95 latency is not measured yet.

llama.cpp is pinned to `b11395` (`7f2dd88b0ac393357ae6a9e1992185a48c20b13b`) in the root README. `llama-completion` is used because the current `llama-cli` ignores `-no-cnv` and blocks at `>` (llama.cpp#27214, open when pinned).

## Todo
- [x] **Stop interactive mode.** Pass `-no-cnv` (or the current non-interactive flag for the pinned llama.cpp). Pin the llama.cpp commit in the README. Accept: `llama-completion` exits on its own after the answer.
- [x] **Kill on timeout.** `kill_on_drop(true)`; timeout currently drops the future and leaves the child running. Accept: no orphan process after a timed-out call.
- [x] **Cold start vs 700 ms.** Loading a model per call cannot meet the cap. Add `LlamaServer` implementation: spawn `llama-server` once, keep it warm, POST `/completion` with `n_predict`, `temperature: 0`, `cache_prompt: true` for the fixed system prompt. Same `Cleaner` trait. Accept: mechanism landed; p95 recorded when measured.
- [ ] **Health and restart.** If the server dies, restart with backoff; meanwhile `clean` returns raw immediately. Accept: kill the server mid-run, subtitles keep flowing.
- [ ] **Stronger acceptance guard.** `accept_cleanup` also checks: no digits added or removed, similar word set (normalized Levenshtein or token overlap threshold), no newline, no leading "Here is". Accept: table test with real bad outputs (translation, added sentence, explanation prefix).
- [ ] **Skip when pointless.** Lines under 3 words or already ending in punctuation with no filler words skip the model. Accept: counter of skipped calls; test for the predicate.
- [ ] **Stop tokens.** `<|im_end|>` and newline as stop; strip stray template tokens from output. Accept: test on output containing `<|im_end|>`.
- [ ] **Prompt not hard-coded for ChatML.** Prompt template per model family in config (06). Accept: second template tested as a pure string fn.
- [ ] **Rule-based fallback.** Optional `RuleCleaner` (strip "um", "uh", doubled words, capitalise, add period). No model, zero latency. Use it when the model path is unset. Accept: table tests.
- [ ] **Model choice doc.** Record measured latency and quality for 2 or 3 small models in this folder. Accept: table with numbers, not opinions.

## Tests
`accept_cleanup`, stop-token stripping, skip predicate, rule cleaner: all pure. Server client tested against a local fake HTTP responder (tokio `TcpListener`), no real model.

## Out of scope
Translation. Rewriting for style. Large models.
