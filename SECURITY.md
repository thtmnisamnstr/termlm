# Security Policy

## Supported Versions

Security fixes are applied to the latest release line.

## Reporting a Vulnerability

Please do **not** open public issues for undisclosed vulnerabilities.

Report privately via GitHub Security Advisories:

- https://github.com/thtmnisamnstr/termlm/security/advisories/new

Include:

- affected version(s)
- reproduction steps
- impact assessment
- proof-of-concept or logs (redacted)

We will acknowledge receipt within 72 hours and provide a remediation timeline.

## Security Posture Notes

- `termlm` executes approved commands in the user's shell; review prompts and approval mode carefully.
- Use `approval.mode = "manual"` for strict environments.
- Keep upgrades on official GitHub release assets and verify checksums/signatures where provided.
