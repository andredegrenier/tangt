# Security policy

## Reporting a vulnerability

Please report security issues privately through GitHub's
[private vulnerability reporting](https://github.com/andredegrenier/tangt/security/advisories/new)
rather than opening a public issue.

## Threat model

tangt is a single-user desktop application. The untrusted input is a `.tangt`
file someone opens: the parser, scheduler, layout, and export pipeline must
never panic, hang, or allocate unboundedly on hostile content. Regression tests
for this live in `crates/tangt-core/tests/hostile_input.rs`; if you find a file
that breaks any of those properties, that is a valid report.

tangt performs no network I/O and contains no `unsafe` code.

## Verifying releases

Release binaries are built by GitHub Actions from a tagged commit. Each archive
ships a `.sha256` checksum and signed build provenance:

```sh
gh attestation verify tangt-<platform>.zip -R andredegrenier/tangt
shasum -a 256 -c tangt-<platform>.zip.sha256
```

Binaries are **not** code-signed or notarized. On macOS, Gatekeeper will refuse
to open the binary until you remove the quarantine attribute
(`xattr -d com.apple.quarantine tangt`) or right-click ▸ Open. On Windows,
SmartScreen shows a warning until the binary accrues reputation. Verify the
attestation before bypassing either warning — or build from source.
