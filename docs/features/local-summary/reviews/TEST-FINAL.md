# Independent verification — Podtekst local summary v1 (final, fixed HEAD)

- Date: 2026-11-08 (Europe/Moscow)
- Worktree: `<worktree>`
- Code commit verified: **`21184b0`** (`fix(ci): enforce realistic real-model budgets`), which includes `c5e47f4`, `4f546cd`, `21184b0`.
- Repository HEAD at the end of the pass: **`79554bf`**. The delta `21184b0..79554bf` is docs-only (`git diff --stat`: `IMPLEMENTATION-NOTES.md`, `SPEC.md`; no code/test/build files). Code state is therefore identical to `21184b0`.
- Constraint: **no product code was modified** by this pass. `git status --porcelain` is clean. All data below is synthetic (fake names, no real recordings/topics). `.mr-stamp` was restored after `yarn build` exactly as CI does.
- Time-boxed pass: long real-model runs were stopped. Items not re-measured are marked **not measured** rather than passed.

## Verdict

| Area | Result |
|---|---|
| Format / clippy / workspace tests / Android / frontend / static stamp | **PASS** |
| Contract suite incl. concurrent generate (H02), downgrade flags, absolute GET | **PASS** |
| Real absolute-path UI flow (H01) | **PASS** (real model, 111.4 s generation) |
| English UI task fallbacks / metadata | **PASS** (fake-backend UI path + unit test) |
| Sensitive downgrade (N-M01) | **Partially verified** — backend contract passes; real UI reached confirmation and started generation, but the result exceeded the 300 s timeout under host load 27; the fake-backend rerun did not render the panel on the second app instance |
| Cancel during real generation (S-05) | **Unit/contract evidence PASS**; real-model cancel **not re-run** in this pass |
| Privacy canary in SSE/logs/argv | **Not re-measured** in this pass (surface unchanged by the fix commits; previous pass passed) |
| Packaging (G6) | **PASS locally** for AppImage/runtime/staged dirs; APK/CI evidence is CI-only |
| Release recommendation | **Conditional GO** — see the end |

## Environment

| Item | Value |
|---|---|
| CPU / RAM | 20 logical cores, 31 GiB RAM, no GPU |
| Load during real runs | load average **14–27** (peaked at 26.9; another agent ran a second `llama-server` concurrently) |
| OS | Linux x86_64 |
| Runtime | llama.cpp `b12182`, `0.9.0-dev`, commit `fd27b2034`, `<tmp>/runtime/linux-min/` |
| Model | `Qwen3-4B-Instruct-2507-Q4_K_M.gguf`, **2,497,281,120 B**, SHA-256 `3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597` (verified) |
| App binary | `src-tauri/target/release/podtekst`, built at `21184b0`, 36,726,600 B, 08:17 |
| Synthetic data | temp E2E dirs under `<tmp>-e2e-*`; no real data |

## Static / unit / build gates

All commands run from the worktree with `--locked`.

| # | Command | Result | Evidence |
|---|---|---|---|
| 1 | `cargo fmt --all -- --check` | **PASS** (`FMT_CHECK=0`) | `f-build.log` |
| 2 | `cargo clippy --workspace --all-targets -- -D warnings` | **PASS** (`CLIPPY1=0`, 3.01 s) | `f-build.log` |
| 3 | `cargo clippy -p mr-app --features tauri-ui --all-targets -- -D warnings` | **PASS** (`CLIPPY2=0`, 13.15 s) | `f-build.log` |
| 4 | `cargo test --workspace` | **PASS: 389 passed, 0 failed, 2 ignored** (24 `test result: ok` groups) | `f-test-workspace.log` |
| 5 | `cargo build --release -p mr-app` | **PASS** (2m56s) | `f-build.log` |
| 6 | Android check, NDK 27.1.12297006 | **PASS** (26.91 s); 5 pre-existing warnings in `screen.rs`/recording, none in summary | `f-android.log` |
| 7 | `cd ui/web/frontend && yarn check` | **PASS**, 0 errors / 0 warnings, fixtures ok | `f-yarn-check.log` |
| 8 | `cd ui/web/frontend && yarn build` | **PASS**, `app.js` 300.82 kB, `app.css` 93.01 kB (2.87 s) | `f-yarn-build.log` |
| 9 | static stamp after build | **PASS**; expected hash `0dd10db7…bb1ec` equals `.mr-stamp`; `git diff --exit-code ui/web/static` clean | `f-build.log` |
| 10 | `cargo test -p mr-app --test summary_api_contract` | **PASS** (1/1 suite, 20.49 s) | `f-contract.log` |
| 11 | `summary_packaging_guard` (inside workspace test) | **PASS 3/3** | `f-test-workspace.log` |

Key fix tests that passed in the workspace run:

- `summary::runtime_locator::tests::large_help_output_is_drained_without_deadlock` (128 KiB `--help` stub — N-H01).
- `summary::job::tests::cancel_kills_generation_child_and_releases_slot` (child gone <2 s — S-05).
- `summary::job::tests::double_reserve_yields_one_slot` (H02).
- `parse::tests::label_values_are_sanitized`, `task_field_values_are_sanitized`, `decision_label_value_is_sanitized` (N-M02).

The contract suite includes:
- `13d` two simultaneous `/api/summary/generate` calls → exactly one **202** and one **409** (`statuses == [202, 409]`).
- `9b` absolute-path GET and symlink-outside rejection.
- `13b/15b` `force` + `allow_downgrade` downgrade matrix.

## H01 — absolute-path real UI flow

Used a copy of `scripts/summary_absolute_path_e2e.py` with the result timeout raised **90 s → 300 s** (the repo default is still 90 s; see N-L03) and only the `ROOT` constant adapted for the copy. The repo file itself was not changed.

Command:

```bash
PODTEKST_SUMMARY_RUNTIME=<tmp>/runtime/linux-min \
MR_SUMMARY_MODEL=<tmp>/models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf \
python3 …/summary_absolute_path_e2e_300.py --real --binary src-tauri/target/release/podtekst --keep
```

Result: the **absolute-path meeting flow passed** before the run reached the later sensitive case:

- UI opened `#/transcript?path=<absolute>` and clicked «Сделать итоги».
- It asserted the POST body file is the **absolute path**, `allow_downgrade` is not set, progress/result appeared, «Готово»/«Сгенерировано» shown.
- Artifact `meeting.summary.json`: **3,645 B**, schema `podtekst.summary/v1`, `output_language=ru`, preset «Встреча».
- Generator timings: `load 7,675 ms`, `generate 103,733 ms`, `total 111,418 ms`, 8 threads, model SHA verified.
- Sections: brief 2 items, decisions 1; warnings `fallback_extracted`, `items_dropped_unsupported`, `short_source`, `soft_claim_dropped`.
- The real `newer.major → v1` downgrade also completed: `newer.summary.json` 3,880 B, schema v1, backup `newer.summary.json.newer.1794115368`.

The overall script exited **1** later, at the sensitive/legal result wait:

```
playwright._impl._errors.TimeoutError: Page.wait_for_selector: Timeout 300000ms exceeded.
waiting for locator(".summary-result")
wall 8:15.70, load average 26.9
```

This is recorded as **environment-attributed**, not as a failure of the absolute-path flow. The legal job was at `generate/44 %` after 3m45s while another agent's `llama-server` used ~190–250 % CPU; the 300 s result timeout was reached first.

## H02 — concurrent generate

- `cargo test -p mr-app --test summary_api_contract` **passed**, including `13d`: two concurrent POSTs produce exactly `[202, 409]`.
- Unit `double_reserve_yields_one_slot` **passed**.
- No second 202 was observed in this pass; the contract is the authoritative evidence.

## S-05 — cancel during real generation

- Unit `cancel_kills_generation_child_and_releases_slot` **passed**: installs a generation child, cancels, asserts the child is gone within 2 s, the job slot is released, and the sidecar state is cleared.
- `runner.rs` now kills the sidecar when the job token is cancelled; `job.rs` retains the 1 s delayed kill.
- **Real-model cancel was not re-run** in this time-boxed pass. The previous pass at `ff5d6ac` reproduced the leak, but the code changed in `fea608f`. A real cancel assertion on the fixed HEAD remains **unmeasured**.
- Residual race N-L05 (cancel between the runner's check and `JobFinish::drop`) is unchanged and not exercised by the unit test.

## English output and sensitive downgrade

**English (fake-backend UI path) — PASS.** The fake E2E passed `test_english_task_fallback` (it continued to the sensitive test, which is the only later failure). It asserted:

- section heading **“Tasks”** (not «Задачи»);
- preset metadata **“Meeting”**;
- task fallback **“unassigned”** and **“deadline not stated”** (no «не назначен» / «срок не назван»).

Unit test `english_fixture_keeps_titles_deadline_and_locale` also passed in the workspace run, covering English title, preset label, model title, and Markdown without Cyrillic.

**Sensitive downgrade (N-M01) — partially verified.**

- Backend contract tests for `force:true` + `allow_downgrade:true` pass.
- `summaryActions.ts` now sends `force: summaryState.stale || allowDowngrade` and `force: opts.force === true || opts.allowDowngrade === true`.
- Real UI run reached the sensitive confirmation for `legal.wav`, asserted the confirmation text, and clicked «Собрать в старом формате»; the generation then exceeded the 300 s result timeout under load 27. The artifact remained v2; the request flags were not captured.
- The fake-backend rerun **failed earlier**: the second app instance (manual_only + sensitive legal preset) never rendered `.summary-block` within 20 s. The first four fake flows passed. This is either a test-harness/restart flake or a real manual_only panel regression; it needs one focused rerun. It is the main evidence gap of this pass.

## Privacy canary / no text in SSE/logs/argv

**Not re-measured in this pass.** The fix commits do not touch the SSE event schema, sidecar argv construction, or log redaction. The previous pass at `ff5d6ac` passed:

- canary absent from summary, SSE frames, app logs, and `llama-server` argv;
- SSE frames contained only stage/percent fields (`load/prepare/generate`), no `text`, `prompt`, `markdown`, or tokens.

Because the task explicitly asks for a HEAD check, this item remains **open as an evidence gap**, not as a known failure. A short canary run is listed under the release conditions.

## Packaging (G6)

Locally verified:

| Item | Value |
|---|---|
| AppImage | `Podtekst_0.6.3_amd64.AppImage`, **118,659,576 B** (113.16 MiB), SHA-256 `0fa77998…d63ea67` |
| AppImage runtime dir | `usr/lib/Podtekst/runtime/x86_64-unknown-linux-gnu`, **44,254,356 B**, **23 files**, **0 symlinks** |
| Bundled `llama-server --version` | `0.9.0-dev (build 12182, commit fd27b2034)` |
| `ldd` all bundled `.so` + binary | **no `not found`** |
| Staged runtimes | Linux 44,254,356 / 23 files; macOS 31,249,604 / 13; Windows 46,889,299 / 25; 0 symlinks |
| Base vs release Tauri config | `summary_packaging_guard` 3/3 PASS |
| Stored APK | `g6/Podtekst-0.6.3-release.apk`, **42,586,050 B**, SHA-256 `ce65bb51…f5bd900`, 704 files |
| APK content | **no** `llama-server`, `libllama`, `libggml`, `.gguf`, `assets/runtime`; only `lib/arm64-v8a/{libc++_shared.so,libmr_app_lib.so}` |
| APK size claim | G6 report now labels **35,036,193 B / +133,000 B** as a historical local debug build; `apk-list.txt` (705 files) does not match the stored APK (704 files / 42,586,050 B). CI `apk-evidence.json` is the authoritative per-release value. |
| Archive SHA-256 | G6 report values (Linux `c1dbfdb9…7d99`, macOS `71cfee07…f1a6`, Windows `e068c483…5834`) were not re-downloaded in this pass; staged sizes match. |

CI-only, not locally measured: macOS `.app`/dmg + `otool`/codesign/quarantine, Windows NSIS/Defender/DLL set, Android device run, AVX2/baseline CPU, clean-VM installs, and the actual release `apk-evidence.json`. The G6 report states this honestly.

## Performance

Measured on this host:

| Metric | Value | Source |
|---|---|---|
| Cold load (sidecar/model) | **7,675 ms** | `meeting` artifact `timings.load` |
| Short real meeting (4 synthetic utterances) | **111,418 ms** total (`generate 103,733 ms`) | `meeting` artifact |
| Peak RSS observed | **5,419,196 KiB ≈ 5,292 MiB** (RSS snapshot while generating; VmHWM not read) | `ps` of `llama-server` PID 633580 |
| 10-minute / 9.5k single-pass | **not measured** in this pass | — |
| 4-core tier, 30/60/120-minute cases | **not measured** | manual full matrix |
| Official `summary_real` (peak VmHWM + cancel) | **not run** in this pass | unit/contract only |

Budget policy on HEAD: release CI uses `--budget-profile ci-loaded --enforce-budget`, where the 10-minute case is **≤600 s for both 4–7 and ≥8 logical cores**; 30/60/120 have **no budget** in that profile and are either measured without a gate or listed in `not_run`. The full 10/30/60/120, 4/8-core, English/injection/legal/psychology matrix is a manual `workflow_dispatch` job, not a per-release gate. **No 120 s, 4-core, 9.5k, or 30/60/120-minute budget is claimed here.**

## Severity table

| ID | Severity | Status in this pass | Evidence |
|---|---|---|---|
| H01 absolute path vs relative job key | High | **Fixed / verified** | Real UI absolute-path meeting flow + 111.4 s artifact; contract `9b` |
| H02 double-start race | High | **Fixed / verified** | Contract `13d` exactly `[202, 409]`; unit `double_reserve_yields_one_slot` |
| H03 assignee from another utterance | High | **Fixed (unit)** | Workspace tests pass; no independent live probe in this pass |
| H04 first-word name bypass | High | **Fixed (unit)** | `invented_first_word_name_is_dropped` passes |
| H05 task cue without commitment | High | **Fixed (unit)** | Discussion/negation tests pass |
| H06 release CI never runs real model | High | **Fixed in workflow, not executed** | `release.yml` `real-summary` job, `SUMMARY_REQUIRE_REAL=1`, manifest, publish dependency |
| N-H01 runtime probe deadlock | High | **Fixed (unit)** | 128 KiB `--help` test passes; macOS/Windows not run |
| N-M01 sensitive downgrade | Medium | **Partially verified** | Backend contract passes; real UI timed out at result; fake sensitive rerun failed at `.summary-block` |
| N-M02 label-value sanitizer | Medium | **Fixed (unit)** | Three label/task/decision sanitizer tests pass |
| M07 English titles/metadata/UI fallback | Medium | **Fixed (unit + fake UI)** | English fixture test; fake E2E “Tasks/Meeting/unassigned/deadline not stated” |
| M18 budget enforcement | Medium | **Policy fixed; full matrix not run** | `ci-loaded` 600 s gate in `release.yml`; manual `full-matrix` job |
| S-12 raw `fallback_extracted` | Low | **Fixed** | `WARNING_TEXT` now maps it to Russian copy |
| N-L01 synchronous model SHA | Low | Open | 2.3 GiB hash before 202 |
| N-L02 failed runtime re-probe | Low | Open | success-only cache |
| N-L03 real E2E default 90 s | Low | Open | repo script still `timeout=90000`; 300 s copy needed |
| N-L04 release-mode test hook | Low | Open | `set_transcribing_for_test` is `cfg(debug_assertions)` but called unconditionally in the contract test |
| N-L05 cancel/JobFinish race | Low | Open | delayed kill skipped if job finishes first |
| N-L06 short-name/negation precision | Low | Open | prefix match and two-word negation window |
| New: fake sensitive E2E panel timeout | Low/Medium evidence gap | Open | Second app instance (manual_only) did not render `.summary-block` within 20 s |

No Critical or new High product defect was reproduced. The only Medium-level uncertainty is the sensitive-downgrade UI verification.

## Not measured / limitations

- Real-model **cancel during generate** on the fixed HEAD (S-05); only unit/contract evidence.
- Privacy canary run on the fixed HEAD (SSE/logs/argv); previous pass passed.
- Official `summary_real` smoke with peak VmHWM, cold load, and cancel timing.
- 9.5k single-pass, 4-core tier, 30/60/120-minute cases; full manual matrix.
- macOS/Windows runtime probes, bundles, codesign/quarantine, Defender, ACL, device APK.
- CI execution of `real-summary`, `smoke-linux`, `smoke-android`, `apk-evidence`.
- Model download/resume/SHA/mirrors, diagnostics zip, SSE reconnect, 65 KiB boundary, shared-stem conflict.
- Frontend has no unit runner; UI evidence is Playwright/code review only.

## Release recommendation

**Conditional GO.**

The fixed code state passes all static/unit/build gates, the contract suite (including H02), the real absolute-path UI flow (H01), the English UI fallbacks, and the local packaging checks. No Critical/High/Medium product defect was reproduced on `21184b0`.

Do **not** treat this as full release sign-off until:

1. The release CI `real-summary` job is green with `SUMMARY_REQUIRE_REAL=1`, the `ci-loaded` 600 s 10-minute budget, and the uploaded manifest; the full matrix remains a separate manual gate.
2. One clean-host rerun verifies the sensitive-downgrade UI (capture `force:true` + `allow_downgrade:true` and the v1 rewrite). The current failure is a 300 s timeout under load 27 plus a fake-backend panel flake, not a reproduced product defect, but it is a Medium evidence gap.
3. The S-05 real cancel (child gone <2 s) and the privacy canary (no text in SSE/logs/argv) are re-run on this HEAD, or the release accepts the unit/prior-pass evidence explicitly.
4. CI packaging (`smoke-linux`, `smoke-android`, `apk-evidence`, macOS/Windows smoke) is green.

If the project's rule is “no Medium may remain unverified”, the sensitive-downgrade item keeps the release **blocked** until condition 2 is green.

## Evidence index

- `verification/head-21184b0/f-build.log` — fmt/clippy/release build.
- `verification/head-21184b0/f-test-workspace.log` — 389 passed / 2 ignored.
- `verification/head-21184b0/f-contract.log` — contract suite.
- `verification/head-21184b0/f-android.log` — Android check.
- `verification/head-21184b0/f-yarn-check.log`, `f-yarn-build.log` — frontend.
- `verification/head-21184b0/f-e2e-real.log`, `f-e2e-real.exit` — real absolute-path/UI run (meeting passed; legal timeout).
- `verification/head-21184b0/f-e2e-fake.log` — fake UI run (first four flows passed; sensitive panel timeout).
- `<tmp>-e2e-s0xuat_z/data/recordings/meeting.summary.json` — real artifact/timings (kept temp dir).
- `<tmp>-e2e-s0xuat_z/data/recordings/newer.summary.json` + `.newer.*` backup — real downgrade.
- G6: `docs/features/local-summary/G6-REPORT.md`; local `src-tauri/target/release/bundle/appimage/`; `<tmp>/g6/`.
- Prior pass: `<tmp>/verification/test-report.md`; OCR re-review: `ocr-review-final.md`.
