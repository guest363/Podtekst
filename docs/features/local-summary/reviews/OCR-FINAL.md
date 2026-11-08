# OCR review — final verification 2 (`feat/local-summary`)

- Workspace: `<worktree>`
- Branch: `feat/local-summary`, HEAD `21184b0`
- Fix commits: `4f546cd`, `c5e47f4`, `21184b0` (previous fixes below `2279400`)
- Previous review: `<tmp>/verification/ocr-review-final.md`
- Review date: 2026-11-08
- Tool: `open-code-review v1.13.6`, delegate preview (`--from 2279400 --to feat/local-summary`, JSON) plus full diff/file inspection and targeted execution.
- Constraint: no repository files were modified. `yarn build` briefly removed the generated `.mr-stamp`; it was restored with `git checkout -- ui/web/static/.mr-stamp` and the tree is clean.

## What was re-run

- `ocr delegate preview --from 2279400 --to feat/local-summary`: 15 changed files, 14 reviewable, 1 excluded (`REAL-MATRIX.md`, read as context). All 14 reviewable files were inspected.
- `cargo test --workspace --locked`: **389 passed, 2 ignored, 0 failed**.
- `cargo test -p mr-summarize --locked`: **122 passed, 1 ignored** (99 unit + 5 + 4 + 4 + 4 + 6).
- `cargo test -p mr-app --lib runtime_locator --locked`: **7 passed**, including `large_help_output_is_drained_without_deadlock`, `hung_probe_is_bounded`, `bad_candidate_does_not_hide_valid_runtime`.
- `cargo clippy --workspace --all-targets --locked`: clean.
- `cargo fmt --all -- --check`: no diff.
- `yarn check`: 0 errors, 0 warnings; fixture check passed.
- `yarn build`: succeeded; generated bundle contains the new English fallbacks and `force/allow_downgrade` logic. Working tree restored clean.
- `python3 scripts/summary_absolute_path_e2e.py --binary src-tauri/target/debug/podtekst` (fake backend): **passed**, including `test_sensitive_downgrade` and `test_english_task_fallback`.
- Real-mode E2E with the branch’s fixed 90 s timeout: failed again at the first `.summary-result` wait (existing Low N-L03, test-harness timeout, not a product error). A copy with 300 s timeouts was still running at the review time-box and is not used as evidence.
- Release workflow YAML parses; job graph, `full-matrix` condition, and `publish.needs` inspected.
- `.github/scripts/summary_real_matrix.py` `parse_runs`/`budget_for` exercised directly; shell script `bash -n` clean.

## Coverage

| Group | Reviewable | Reviewed | Skipped |
|---|---:|---:|---:|
| CI, workflows and scripts | 4 | 4 | 0 |
| `mr-app` Rust (config, pipeline, runtime locator) | 3 | 3 | 0 |
| `mr-summarize` Rust and parser tests | 3 | 3 | 0 |
| Frontend source/build stamp/static bundle | 4 | 4 | 0 |
| **Total** | **14** | **14** | **0** |

Coverage: **100%** of OCR-reviewable files. `docs/features/local-summary/REAL-MATRIX.md` was read as excluded context.

## Requested findings

### N-H01 (High) — Fixed

`runtime_locator.rs:232-239` now starts a reader thread for each pipe before waiting, so `--help`/`--version` cannot fill a pipe and block the child. `read_all` (`:273-291`) always drains, retaining at most 4 MiB; `join_reader` (`:295-307`) bounds the join so a grandchild holding a pipe cannot turn the 3 s probe timeout into an unbounded wait. Timeout and `try_wait` error paths kill, reap, and join both readers (`:245-260`).

Evidence: `large_help_output_is_drained_without_deadlock` (`:485-500`) produces 131,072 bytes (above the Linux 64 KiB pipe buffer) and completes discovery; it fails the old “wait then read” logic. `hung_probe_is_bounded` still passes. The logic is platform-neutral (two threads on both Unix and Windows). No deadlock was observed.

Residual risk (Low): if a probe leaves a grandchild holding a pipe, the reader thread is detached after the bounded join; this is a narrow resource leak, not a hang.

### N-M01 (Medium) — Fixed

`summaryActions.ts:82-90` now carries the downgrade intent through the sensitive confirmation (`force: summaryState.stale || allowDowngrade`), and `:181-186` sends `force: true` whenever `allow_downgrade` is requested. `rebuildInOldFormat` still sets `pendingDowngrade` before opening the sensitive confirmation. This satisfies the backend’s “both flags” preflight (`job.rs:226-235`) and commit check (`pipeline.rs:391-397`).

Evidence: fake E2E `test_sensitive_downgrade` (`scripts/summary_absolute_path_e2e.py:393-423`) creates a newer-major legal artifact, clicks “Собрать в старом формате”, asserts the sensitive confirmation appears without a premature generate request, then asserts the request contains `preset_id=legal`, `confirm_sensitive=true`, **`force=true` and `allow_downgrade=true`**, and that the artifact is rewritten as `podtekst.summary/v1` with a `.newer.*` backup. The fake E2E passed. The real-mode E2E reached the later phase but did not finish inside the time-box; the fake assertion is the deterministic evidence.

### N-M02 / M06 (Medium) — Fixed

`parse.rs:121-126` now applies `contains_injection` to every structured label value before the label match; `:161-168` applies it to cleaned item text before `split_quote`/`parse_task`; continuation (`:190-197`) and fallback (`:212-224`) branches retain their checks; `parse_flat` already sanitized (`:275-285`). The marker list adds `<<<data_`, `_begin>>>`, `_end>>>` (`:498-500`). Because task fields are parsed only from already-checked text, `Кто:`, `Срок:`, `Who:`, `Due:` values are covered as well.

Evidence: `label_values_are_sanitized`, `task_field_values_are_sanitized`, and `decision_label_value_is_sanitized` (`parse/tests.rs:207-252`) assert dropped `<script>`, `https://`, `<img>` and nonce-like values, with one `InjectionSuspected` count for each representative path. All pass. No remaining bypass was found in label, item, continuation, task-field or flat-parser paths.

### M07 (Medium) — Fixed

`PresetInfo::from_preset` now selects `label_en` for English (`artifact.rs:175-185`); `pipeline.rs:328` passes the output language and `:340-344` uses `MODEL_TITLE_EN`; `SummaryResult.svelte:39-43` localizes the “unassigned”/“deadline not stated” task fallbacks from `output_language`; `artifact/markdown.rs` already localized titles, metadata, task labels and the disclaimer.

Evidence: `english_fixture_keeps_titles_deadline_and_locale` now asserts `artifact.preset.label == "Meeting"`, the English model title, `Preset: «Meeting»`, and zero Cyrillic characters in the English Markdown. It passes. Fake E2E `test_english_task_fallback` asserts the rendered UI shows “unassigned”/“deadline not stated”, the `Tasks` section and `Meeting` metadata, with no Russian fallbacks. It passes.

### M18 (Medium) — Fixed as a release gate; full matrix accepted as a documented limitation

The release `real-summary` job now runs the 10-minute real case with `--budget-profile ci-loaded --enforce-budget` (`release.yml:304-309`). The manifest step fails unless a passed 10-minute case has `budget_met=true` and `budget_enforced=true` (`:350-355`), and `publish` still needs `real-summary` (`:906`). The `ci-loaded` profile is an explicit measured ceiling: 600 s for 10 minutes at both tiers, with 30/60/120 deliberately unbudgeted (`summary_real_matrix.py:40-60`). `REAL-MATRIX.md` documents that 600 s is not a SPEC promise and records the 595.68 s loaded measurement.

The full `10/30/60/120`, 4/8-core, English/injection/legal/psychology matrix remains **manual only** (`workflow_dispatch`, `full_matrix=true`) and was **not executed** in this review or in the release workflow. Quick runs list every heavy/quality case in `not_run` with an explicit reason; the manifest carries `not_run`. This is a **documented acceptance limitation**, not a code bug. It is not silently downgraded: the 120-minute case has no measured evidence and no budget claim.

Operational Low: the `full-matrix` job has `timeout-minutes: 360` (`release.yml:455`) while `--full --cores 4,8 --runs 1/3` can consume 240 min per tier at the enforced maxima (5 short cases × 3 runs × 600 s + 3 heavy cases × 1800 s), i.e. 480 min for both tiers plus startup overhead. A full run on an 8+ core runner can therefore be killed before writing evidence. This is a manual-CI timeout issue, not a product defect; raise/split the timeout before treating the full matrix as a repeatable gate.

## Other previous findings

All previous Highs and Mediums not listed above remain fixed; no regression was found in the changed code.

| Previous item | Status |
|---|---|
| H01–H06 | Fixed (unchanged from final review) |
| M01–M05 | Fixed (unchanged) |
| M08–M17 | Fixed (unchanged) |
| S-05, S-08, S-11 | Fixed (unchanged) |
| M06, M07, M18 | M06/M07 fixed; M18 release gate fixed, full matrix accepted limitation |

Open Low backlog remains: N-L01 (synchronous model SHA preflight), N-L02 (failed probes re-probed), N-L03 (real E2E 90 s timeout, reconfirmed), N-L04 (release-mode test hook), N-L05 (cancel race window), N-L06 (short-name/negation precision). These are not release blockers under the no-High/Medium rule.

## New findings

### High / Medium

**None.** No new Critical, High or Medium finding was introduced by `4f546cd`, `c5e47f4` or `21184b0`.

### Low

**N-L07 — full-matrix job timeout has no headroom (manual CI only).** `release.yml:455` gives the full matrix 360 minutes; the documented command can exceed that at the enforced budgets (see M18 above). This does not affect the release gate, which runs only the 10-minute case. Fix by raising/splitting the job timeout or reducing runs before relying on the full matrix as evidence.

## Test assertion audit

- Asserted by executed tests: N-H01 (`large_help_output_is_drained_without_deadlock`, `hung_probe_is_bounded`), N-M01 (fake E2E body/artifact assertions), N-M02/M06 (three new parser tests), M07 (pipeline English test + fake E2E UI test), M18 (script-level `budget_for`/gate inspection; release workflow not executed).
- Code-reviewed but not executed here: Windows/macOS runtime probe, Windows ACL (M08), Windows packaging (M15), Android APK evidence (S-11), release workflow itself, full 10/30/60/120 matrix.
- Existing test gaps: the >64 KiB regression test covers stdout only, not stderr (the same `read_all` code handles both); the real-mode E2E still uses a fixed 90 s timeout (N-L03); UI meta labels “Модель/Шаблон” remain Russian chrome while values are localized (cosmetic).

## Release verdict

**Pass with a documented acceptance limitation.** There are **no open Critical, High or Medium** findings under the spec rule. The release gate is materially real: it requires a live 10-minute case, a pinned runtime/model, `--enforce-budget`, `budget_met=true` and `budget_enforced=true`, and `publish` depends on it.

The only unverified acceptance item is the **full 10/30/60/120, 4/8-core, English/injection/legal/psychology matrix**, which is manual, explicitly `not_run` in quick evidence, and must be recorded as not executed. That is a documented acceptance limitation, not a code bug. It is not silently downgraded, and the 120-minute case must not be described as measured or budget-compliant.

## Final counts

| Severity / disposition | Count |
|---|---:|
| Previous findings fixed | 26 |
| Previous findings fixed with documented limitation | 1 (M18 full matrix) |
| Previous findings not fixed | 0 |
| New Critical | 0 |
| New High | 0 |
| New Medium | 0 |
| New Low | 1 (N-L07) |
| Open Low backlog (previous + new) | 7 |

## Top remaining items

1. Record the full matrix once on an 8+ core runner, or explicitly accept and sign off the unrun 120-minute case; raise the 360-minute `full-matrix` timeout first.
2. Fix the real E2E’s fixed 90 s result timeout (N-L03) so the real path is usable without a patched copy.
3. Clear the remaining Low backlog: model SHA preflight latency, failed-runtime re-probes, release-mode test hook, cancel race, short-name/negation precision.
4. Watch the first CI run of the 600 s `ci-loaded` budget; it has only 0.7% headroom over the 595.68 s local measurement.
5. Optional hardening: exercise >64 KiB stderr in the probe regression test and localize the remaining Russian meta chrome for English artifacts.
