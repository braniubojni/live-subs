# 04 Cleanup

## Goal
Remove fillers, fix punctuation and case on a final line within a hard 700 ms budget. Never delay or corrupt the subtitle. On any doubt, show the raw line.

## Current state
[src/cleanup.rs](../../src/cleanup.rs): spawns `llama-cli` per line with ChatML prompt, `-n 64 --temp 0 -c 512 -ngl 99`, 700 ms timeout, `accept_cleanup` rejects empty or more than 2x length.

## Todo
- [ ] **Stop interactive mode.** Pass `-no-cnv` (or the current non-interactive flag for the pinned llama.cpp). Pin the llama.cpp commit in the README. Accept: `llama-cli` exits on its own after the answer.
- [ ] **Kill on timeout.** `kill_on_drop(true)`; timeout currently drops the future and leaves the child running. Accept: no orphan process after a timed-out call.
- [ ] **Cold start vs 700 ms.** Loading a model per call cannot meet the cap. Add `LlamaServer` implementation: spawn `llama-server` once, keep it warm, POST `/completion` with `n_predict`, `temperature: 0`, `cache_prompt: true` for the fixed system prompt. Same `Cleaner` trait. Accept: p95 under 400 ms on a fixture set, recorded in the PR.
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
