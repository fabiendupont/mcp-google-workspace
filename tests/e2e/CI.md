# Running the E2E harness in CI

**Rationale (one line):** `tests/e2e/run.sh` is the only test that exercises the
real Google Workspace API path end to end, so unless a workflow runs it, its
`PASS`/`FAIL` bookkeeping cannot detect anything and silent regressions are free.

**Owner:** `.github/workflows/` is not owned by the tests/ area. A workflow owner
must paste the job below into `.github/workflows/ci.yml` (or a new
`.github/workflows/e2e.yml`) and add the trigger it expects.

## Job snippet to paste

```yaml
  # Paste into .github/workflows/e2e.yml (or ci.yml). Requires the workflow to
  # already declare `on: workflow_dispatch` and/or a `schedule:` entry, because
  # this job needs live Google credentials and a paid judge model.
  e2e:
    name: E2E (live Google Workspace)
    if: github.event_name == 'workflow_dispatch' || github.event_name == 'schedule'
    runs-on: ubuntu-latest
    timeout-minutes: 90
    permissions:
      contents: read
    env:
      CARGO_TERM_COLOR: always
      # public identifiers, stored as repository variables
      GWS_TEST_FOLDER_ID: ${{ vars.GWS_TEST_FOLDER_ID }}
      GWS_PROJECT_ID: ${{ vars.GWS_PROJECT_ID }}
      # service-account credentials, written to disk in the prep step
      GOOGLE_APPLICATION_CREDENTIALS: ${{ github.workspace }}/.ci/credentials.json
      # optional: defaults to the documented dummy token, which the harness hashes
      # into a generated navra config at startup
      MCPD_TOKEN: ${{ secrets.MCPD_TOKEN }}
      # /tmp has a quota on some runners and the harness writes its server logs
      # and the generated navra config there; give it a writable directory.
      TMPDIR: ${{ github.workspace }}/.ci/tmp
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
      - uses: dtolnay/rust-toolchain@1c76d1459064d8f3d2b221b01d84c1ecc8a260fe # 1.120.0
        with:
          toolchain: 1.120.0
      - uses: Swatinem/rust-cache@d1527c8f67ef9af357290475d4af4b0fab8f12e0 # v2.9.3

      - name: Prepare credentials and writable TMPDIR
        env:
          GWS_CREDENTIALS_JSON: ${{ secrets.GWS_CREDENTIALS_JSON }}
        run: |
          set -euo pipefail
          mkdir -p "$TMPDIR" .ci
          printf '%s' "$GWS_CREDENTIALS_JSON" > "$GOOGLE_APPLICATION_CREDENTIALS"
          chmod 600 "$GOOGLE_APPLICATION_CREDENTIALS"

      # navra is not published on crates.io. Point NAVRA_INSTALLER_URL at the
      # pinned installer (internal artifact / tarball + checksum) before enabling
      # this job; the harness fails hard when navra is missing.
      - name: Install navra
        env:
          NAVRA_INSTALLER_URL: ${{ vars.NAVRA_INSTALLER_URL }}
        run: |
          set -euo pipefail
          test -n "$NAVRA_INSTALLER_URL"
          curl -fsSL "$NAVRA_INSTALLER_URL" | sh
          navra --version

      - name: Install PyYAML (rubric parsing)
        run: python3 -m pip install --disable-pip-version-check pyyaml

      - name: Build release binary
        run: cargo build --release --locked

      - name: Build container image
        run: docker build -t mcp-google-workspace:ci .

      # One model x one scenario keeps the job inside the timeout. The local
      # gemma/qwen models need Ollama and a large runner, hence --skip-ollama;
      # the Claude model goes through the Vertex config in navra.toml.
      - name: Run E2E harness
        run: |
          set -euo pipefail
          bash tests/e2e/run.sh \
            --skip-build \
            --skip-ollama \
            --skip-gws \
            --model claude-sonnet-4-5@20250929 \
            --scenario full-e2e

      - name: Upload E2E logs
        if: always()
        uses: actions/upload-artifact@v4 # TODO: pin to a commit SHA like the other jobs
        with:
          name: e2e-results
          path: |
            tests/e2e/results/
            ${{ env.TMPDIR }}/*.log
          if-no-files-found: ignore
          retention-days: 14
```

## Required configuration

| Name | Kind | Notes |
| --- | --- | --- |
| `GWS_TEST_FOLDER_ID` | repository **variable** | Drive folder the scenarios write into |
| `GWS_PROJECT_ID` | repository **variable** | Google Cloud project used by policy + Vertex |
| `NAVRA_INSTALLER_URL` | repository **variable** | pinned navra installer/tarball |
| `GWS_CREDENTIALS_JSON` | **secret** | service-account JSON, written to `GOOGLE_APPLICATION_CREDENTIALS` |
| `MCPD_TOKEN` | **secret**, optional | navra auth token; omit to use the dummy token |

`gws` (the Google Workspace CLI used for test-folder cleanup) is not installed in
the snippet, so the job passes `--skip-gws`; that prints `SKIP: prerequisite: gws
(--skip-gws)` and the summary records it. Install `gws` and drop the flag if you
want deterministic cleanup between runs.

## Notes

- `TMPDIR` matters: the harness keeps `mcp-e2e-server.log`, `navra-e2e-serve.log`
  and the generated navra config there, and the default `/tmp` quota has caused
  false failures in this repo.
- No token hash is committed. `navra.toml` / `navra-9320.toml` contain the
  sha256 of the documented dummy token `e2e-test-token-not-a-secret`; the harness
  copies the config to `$TMPDIR/navra-e2e-generated.toml` with
  `sha256($MCPD_TOKEN)` substituted, and deletes it on exit.
- The harness is fail-fast per assertion but keeps running the rest of the
  matrix, then exits non-zero from its `EXIT` trap: parse `=== Summary ===` /
  `Total: <n>  Pass: <n>  Fail: <n>` (unchanged) plus the new
  `Skipped: <n>` line and per-step `PASS`/`FAIL`/`SKIP`/`INFO` list.
