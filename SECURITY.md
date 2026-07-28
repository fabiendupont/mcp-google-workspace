# Security Policy

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.7.x   | Yes       |
| < 0.7   | No        |

## Reporting a vulnerability

If you find a security vulnerability, please report it privately via
[GitHub Security Advisories](https://github.com/fabiendupont/mcp-google-workspace/security/advisories/new).

Do not open a public issue for security vulnerabilities.

You should receive a response within 72 hours. If the vulnerability is confirmed,
a fix will be released as a patch version and credited in the release notes.

## Security model

This project enforces access control through a JSON policy engine. See the
[security documentation](https://fabiendupont.io/mcp-google-workspace/docs/security/model/)
for details on the defense-in-depth model, credential chain, and policy evaluation order.
