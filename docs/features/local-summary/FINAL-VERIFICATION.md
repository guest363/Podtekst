# Local summary v1 — final verification summary

Date: 2026-11-08. Branch `feat/local-summary`; product code at `21184b0`, final docs/test
follow-ups after it. No real recordings or personal data were used.

## Verdict

- **OpenCodeReview final:** 0 Critical, 0 High, 0 Medium; 1 documented acceptance limitation
  (full 10/30/60/120-minute manual matrix not run) and a Low backlog.
- **Static/unit:** 389 tests passed, 2 ignored; fmt/clippy workspace and `tauri-ui` clean;
  Android check passes.
- **Frontend:** `yarn check` 0 errors/warnings; `yarn build`; static stamp committed.
- **Real absolute-path UI:** meeting flow passed with the real 4B model; 4 sections,
  15 refs, progress/result persisted.
- **Real cancel:** generate → cancel → `llama-server` gone, no artifact.
- **Sensitive downgrade:** fake-backend E2E passes (`force` + `allow_downgrade`, v1 rewrite,
  `.newer` backup); real legal generation is slow under load but the flags are captured.
- **Packaging:** Linux AppImage 118,659,576 B; runtime 44,254,356 B / 23 files; no weights;
  headless missing-runtime returns 503; Android APK contains no runtime/model.
- **Release rule:** no High/Medium code finding remains. Full long-meeting evidence and
  macOS/Windows/Android device smoke are CI-only/manual, as documented below.

## Measured (shared host, pessimistic)

- Short real meeting: 111–146 s total; cold load 2.5–7.7 s; peak RSS ~5.3 GiB.
- 10-minute synthetic: 595.7 s under load; `ci-loaded` release budget 600 s (not the 120 s
  SPEC reference). 30/60/120-minute, 4-core and English/legal long cases are manual.
- Full matrix command: `python3 .github/scripts/summary_real_matrix.py --full --cores 4,8
  --runs 1/3 --budget-profile ci-loaded --enforce-budget` (`REAL-MATRIX.md`).

## Known limitations

- Full long-meeting matrix is not run on every release; 30/60/120-minute budgets are not
  claimed. 120 s / 4-core SPEC values are reference targets only.
- Held-out classifier set is unavailable; v1 ships heuristic «Предположение» with manual
  override (D15), no accuracy claim.
- macOS/Windows runtime/bundle/codesign/Defender and Android device smoke are CI-only.
- Windows ACL/per-OS orphan behavior is not executed on live Windows here.
- Minor task text noise/duplicate deadline phrases can occur; warnings are surfaced.
- Low backlog: synchronous SHA preflight, failed-runtime re-probe, release-mode test hook,
  narrow cancel/JobFinish race, short-name/negation precision.
