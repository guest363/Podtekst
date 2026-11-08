# Local summary v1 — implementation notes and honest limits

This file records where the shipped implementation intentionally differs from the
aspirational parts of `SPEC.md` and what was actually measured. It is part of the v1
deliverable; no real recordings, names or transcript text are used.

## Shipped architecture

- `mr-summarize` pure domain crate: presets, heuristic classifier, tolerant
parser, deadline extraction, grounding/soft-claims, deterministic structured fallback,
artifact/DTO, chunking and prompt contracts.
- `mr-app::summary`: one atomic job slot, sidecar lifecycle, model SHA/`/props` checks,
memory/disk guards, private artifacts, stale/newer-major handling, HTTP API and one SSE bus.
- Runtime: pinned llama.cpp `b12182` (`fd27b2034`), pruned resources directory; CPU-only
(`--n-gpu-layers 0`); Linux AppImage bundles it, Android does not.
- Model: Qwen3-4B-Instruct-2507 Q4_K_M (2.33 GiB, Apache-2.0), downloaded explicitly with
SHA-256/resume; not a core ASR model.
- v1 is read-only: no summary editing, token streaming, summary DOCX, custom presets,
Android summary, or 4B classifier button.

## Classification / D15

The v1 classifier is a deterministic heuristic, not a neural model. It returns instantly,
costs no download/RAM, and is shown as «Предположение» with the manual picker always
available. The 9/9 synthetic result is in-sample and is not an accuracy claim. A held-out
30–50 meeting set is not available in this build; `manual_only` is implemented as an
operator fallback if a future test fails, not as the v1 default.

## Generation safeguards added during E2E

- One-shot structure example + content-first prompt; repair only when the model returns
zero parseable items.
- Tolerant parser (`-`, `*`, numbered, `Задача:`/`Срок:`, bare headings, continuation).
- Deterministic fallback from source utterances for missing decisions/tasks/questions;
  exact source text/refs, no new facts.
- Deterministic deadline extraction/reconciliation; task cue gating; assignee only from
  support/task text or the best-ref speaker; first-token/hard-fact and soft-claim checks
  with negation.
- Section-aware task formatting; cap prioritizes tasks/decisions, then questions, then a
  short brief (max 4).
- Every output path is sanitized for injection/HTML/URL/nonce markers; SSE/logs/argv carry
  no transcript/prompt text.
- Model file SHA and `/props` alias/slots/context are verified before generation.

## Measured performance (real 4B, CPU, synthetic data)

Hardware: i9-13900H, 20 logical threads, 31 GiB RAM, no GPU. Host was shared by other agents
(load average often 16–20); absolute times are pessimistic.

| Case | Measured |
|---|---|
| Cold load, 8k | 2.5–7.3 s depending on load |
| 5k synthetic meeting | 126–146 s in app E2E; 65 s isolated 8-thread run |
| 1.1k synthetic meeting | 141 s official `summary_real` under load |
| 10-minute / 9.5k synthetic | 595.7 s under load, budget `ci-loaded` 600 s; the SPEC 120 s reference is **not** met on 4–7 core/loaded hosts |
| Peak RSS 8k | 5.4 GiB (`VmHWM`), 12 GiB total / 6.5 GiB free guard |
| Cancel | 1.6–1.8 s; sidecar gone; no artifact |
| Runtime dir | Linux 44.25 MB, 23 files; AppImage 118.66 MB; APK no runtime/weights |

The release CI enforces the 10-minute `ci-loaded` <=600 s profile. The full
10/30/60/120-minute, 4/8-core, English/injection/legal/psychology matrix is a manual
`workflow_dispatch` gate (`REAL-MATRIX.md`); it is not run on every release. No 120 s or
4-core long-meeting claim is made.

## Platform status

- Linux x86_64: full local E2E, absolute-path UI, AppImage/headless smoke pass.
- macOS arm64 / Windows x86_64: code/CI paths and packaging manifest present; live
  runtime/Defender/codesign smoke is CI-only and not executed locally.
- Android: summary unsupported by design (501); APK contains no runtime/model.
- iOS/mobile web: responsive UI; no iOS runtime test.

## Known limitations / Low backlog

- Minor task text noise/duplicate deadline phrase can appear; warnings are surfaced.
- Full long-meeting quality and 4-core/120-minute budgets remain manual.
- Held-out classifier accuracy is unvalidated; manual override is mandatory.
- Frontend has no unit runner; M14/M16 are code/type checked plus Playwright paths.
- Windows ACL and per-OS orphan behavior are tested on Linux or Windows-only compile,
  not on a live Windows machine in this workspace.
- Runtime is a nightly pinned build; changing the pin requires a new smoke.
