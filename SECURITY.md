# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in Wheelhouse, please report it
responsibly by emailing **security@wheelhouse.paris**.

Do **not** open a public GitHub issue for security vulnerabilities.

## Response Timeline

- **Acknowledgement**: Within 48 hours
- **Assessment**: Within 1 week
- **Fix**: Depends on severity; critical issues addressed immediately

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.x     | Current development — security fixes applied to latest |

## Security Design

- Broker binds to `127.0.0.1` only — localhost is a security invariant, not a default (ADR-001)
- Secrets are never committed to git (NFR-S2)
- All release artifacts include SLSA provenance attestation
