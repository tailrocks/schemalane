# Plan 010: Pin the Flyway-compatible checksum with golden-value and property tests

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat dd0d79d..HEAD -- schemalane-core/src/lib.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: tests
- **Planned at**: commit `dd0d79d`, 2026-07-03

## Why this matters

The checksum is the drift/skip oracle: it decides whether a migration is "already applied" (skip), "changed" (blocked as ChecksumMismatch), or "pending". Byte-identical compatibility with Flyway's `ChecksumCalculator` is an explicit promise (SCHEMALANE_SPEC.md §6.3) — users pointing schemalane at an existing `flyway_schema_history` depend on it. Today **no test asserts a single checksum value**, its line-splitting behavior, or the `u32→i32` reinterpretation. Any regression silently reclassifies entire fleets: phantom drift (blocked deploys) or — worse — a changed migration hashing equal and being skipped.

## Current state

- `schemalane-core/src/lib.rs`, `calculate_checksum` (lines 1588–1603):

  ```rust
  fn calculate_checksum(script: &str, bytes: &[u8]) -> Result<i32, SchemalaneError> {
      let text = std::str::from_utf8(bytes).map_err(|err| {
          SchemalaneError::Validation(format!(
              "migration {script}: content is not valid UTF-8 (invalid byte at offset {}): {err}",
              err.valid_up_to()
          ))
      })?;
      let mut hasher = Hasher::new();
      // `str::lines()` matches BufferedReader.readLine() for files that don't
      // contain lone `\r` characters: splits on `\n` or `\r\n`, excludes the
      // terminator, no trailing empty line for files that end with a newline.
      for line in text.lines() {
          hasher.update(line.as_bytes());
      }
      Ok(i32::from_be_bytes(hasher.finalize().to_be_bytes()))
  }
  ```

  `Hasher` is `crc32fast::Hasher` (IEEE polynomial — same as Java's `java.util.zip.CRC32`). Call sites: SQL discovery (line 725), Rust discovery (line 766).

- Spec contract (SCHEMALANE_SPEC.md §6.3): CRC-32 per line over UTF-8 bytes, terminator excluded, `(int) crc32.getValue()` reinterpret; documented deviations (recorded tradeoffs, do NOT "fix"): lone `\r` and BOM handling.

- The test module (`mod tests`, line 1916) does not import or exercise `calculate_checksum` at all.

- The function is private; tests live in the same file's `#[cfg(test)]` module, so direct calls are fine.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Generate golden values (independent impl) | see Step 1 | prints integers |
| Unit tests | `cargo test -p schemalane-core --locked checksum` | all new tests pass |
| Full gate | `cargo fmt --all -- --check && cargo clippy --workspace --locked --all-targets --all-features -- -D warnings && cargo test --workspace --locked` | exit 0 |

## Scope

**In scope** (the only files you should modify):
- `schemalane-core/src/lib.rs` (test module only — production code unchanged)

**Out of scope** (do NOT touch, even though they look related):
- `calculate_checksum` itself — if a golden value disagrees, that's a STOP, not a code edit.
- Lone-`\r` / BOM behavior — documented deviations.

## Git workflow

- Branch: `advisor/010-checksum-golden-tests`
- Suggested commit: `Add golden-value tests for Flyway-compatible checksum`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Generate golden values from an independent implementation

The point is cross-implementation agreement, not self-consistency. Compute expected values with Python's `zlib.crc32` (also IEEE CRC-32), implementing the *spec sentence* (per-line, no terminators, signed reinterpret) independently of the Rust code:

```sh
python3 - <<'EOF'
import zlib

def flyway_checksum(text: str) -> int:
    crc = 0
    # readLine() semantics for files without lone \r:
    for line in text.replace("\r\n", "\n").split("\n"):
        crc = zlib.crc32(line.encode("utf-8"), crc)
    # Java: (int) crc32.getValue()  — reinterpret as signed 32-bit
    return crc - 0x1_0000_0000 if crc >= 0x8000_0000 else crc

cases = {
    "empty":            "",
    "single_no_nl":     "CREATE TABLE cake (id INT);",
    "single_with_nl":   "CREATE TABLE cake (id INT);\n",
    "two_lines_lf":     "CREATE TABLE cake (\n    id INT\n);",
    "two_lines_crlf":   "CREATE TABLE cake (\r\n    id INT\r\n);",
    "utf8":             "-- caké 🍰\nSELECT 'schöne Grüße';\n",
}
for name, text in cases.items():
    print(f"{name}: {flyway_checksum(text)}")
EOF
```

**Caveat on `split("\n")` vs `str::lines()`**: for text **ending** in `\n`, Python's `split` yields a final empty string that `readLine()`/`lines()` do not. Hashing an extra empty line is a no-op for CRC-32 (zero bytes), so the values still agree — this is why `single_with_nl` must equal `single_no_nl` in the output. Confirm that equality holds in the printed values; it is itself one of the properties under test.

**Verify**: the script prints six integers; `single_no_nl == single_with_nl`; `two_lines_lf == two_lines_crlf`; `empty == 0`.

### Step 2: Write the golden + property tests

Append to `mod tests` in `schemalane-core/src/lib.rs` (add `calculate_checksum` to the `use super::{…}` list):

```rust
// ── Checksum: Flyway compatibility (spec §6.3) ──────────────────────

/// Golden values computed with an independent CRC-32 implementation
/// (python3 zlib.crc32) of the spec §6.3 algorithm. If these ever fail,
/// the checksum changed and EVERY stored history checksum is affected —
/// do not update the constants without understanding why.
#[test]
fn checksum_golden_values() {
    let cases: &[(&str, &[u8], i32)] = &[
        ("empty.sql", b"", 0),
        ("single.sql", b"CREATE TABLE cake (id INT);", <FILL>),
        ("single_nl.sql", b"CREATE TABLE cake (id INT);\n", <FILL>),
        ("two_lf.sql", b"CREATE TABLE cake (\n    id INT\n);", <FILL>),
        ("utf8.sql", "-- caké 🍰\nSELECT 'schöne Grüße';\n".as_bytes(), <FILL>),
    ];
    for (script, bytes, expected) in cases {
        assert_eq!(
            calculate_checksum(script, bytes).expect("checksum"),
            *expected,
            "golden mismatch for {script}"
        );
    }
}

#[test]
fn checksum_line_endings_are_equivalent() {
    let lf = calculate_checksum("a.sql", b"line one\nline two\n").unwrap();
    let crlf = calculate_checksum("a.sql", b"line one\r\nline two\r\n").unwrap();
    assert_eq!(lf, crlf, "\\n and \\r\\n must hash identically");
}

#[test]
fn checksum_trailing_newline_is_irrelevant() {
    let with_nl = calculate_checksum("a.sql", b"SELECT 1;\n").unwrap();
    let without = calculate_checksum("a.sql", b"SELECT 1;").unwrap();
    assert_eq!(with_nl, without);
}

#[test]
fn checksum_line_terminator_bytes_are_excluded() {
    // If terminators were hashed, these would differ from the joined form.
    let joined = calculate_checksum("a.sql", b"ab").unwrap();
    let split = calculate_checksum("a.sql", b"a\nb").unwrap();
    assert_ne!(joined, split, "distinct line structure must differ");
    // and a pure structural check: "a\nb" == "a\r\nb"
    assert_eq!(split, calculate_checksum("a.sql", b"a\r\nb").unwrap());
}

#[test]
fn checksum_can_be_negative_i32() {
    // Find/verify one fixture whose CRC-32 exceeds i32::MAX to lock the
    // Java (int) reinterpretation. The Step 1 script prints signed values —
    // pick any case that printed negative and assert it here.
    let value = calculate_checksum("neg.sql", b"<FILL: content with negative golden>").unwrap();
    assert!(value < 0, "expected negative reinterpreted checksum, got {value}");
    assert_eq!(value, <FILL>);
}

#[test]
fn checksum_rejects_non_utf8() {
    let err = calculate_checksum("bad.sql", &[0xff, 0xfe, b'a']).expect_err("non-UTF-8 must fail");
    assert!(err.to_string().contains("not valid UTF-8"), "got: {err}");
}
```

Replace each `<FILL>` with the Step 1 outputs. For `checksum_can_be_negative_i32`: if none of the six cases printed negative, extend the Python script with more inputs (e.g. append digits to a line) until one is negative, and use that exact content.

**Verify**: `cargo test -p schemalane-core --locked checksum` → all new tests pass on the FIRST run (no constant-tweaking; see STOP conditions).

### Step 3: Full gate

**Verify**: fmt + clippy + `cargo test --workspace --locked` exit 0.

## Test plan

The steps above ARE the test plan: 5 golden values, 4 structural properties (`\n`≡`\r\n`, trailing-newline irrelevance, terminator exclusion, negative reinterpret), 1 error path (non-UTF-8). Model formatting/placement on the existing checksum-adjacent tests in the same module (e.g. `parse_sql_migration_*`).

## Done criteria

- [ ] `cargo test -p schemalane-core --locked checksum` → ≥6 tests pass
- [ ] `git diff` touches ONLY the `#[cfg(test)]` module (production `calculate_checksum` byte-identical)
- [ ] fmt/clippy/workspace tests exit 0
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Any golden value disagrees between the Python reference and the Rust function — that is either a real compatibility bug (jackpot: report inputs + both values) or a reference-script mistake; **never** "fix" by copying the Rust output into the constant.
- `calculate_checksum` was changed/moved since `dd0d79d` (drift) — the goldens must be validated against the spec §6.3 text, not the new code.
- If genuine cross-validation against real Flyway is requested (running the Flyway CLI in Docker against fixture files and reading `flyway_schema_history.checksum`), treat that as an extension: it's the strongest possible test but needs network + a Flyway image; note it as follow-up rather than blocking this plan.

## Maintenance notes

- These constants are a compatibility contract. A PR that "updates the golden values" is claiming to break Flyway compatibility for every existing deployment — reviewers must treat that as a semver-major event, not a test fix.
- Deferred follow-up: true differential test against the Flyway CLI (Docker) in the integration suite; and lone-`\r`/BOM handling if Flyway parity for those is ever promised (currently documented deviations).
