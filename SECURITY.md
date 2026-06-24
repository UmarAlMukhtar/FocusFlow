# Security Policy

## Supported Versions

FocusFlow is currently in early development. Security updates are provided for the latest public release only.

| Version | Supported |
| ------- | --------- |
| 0.1.x   | Yes       |
| < 0.1   | No        |

## Reporting a Vulnerability

If you discover a security vulnerability in FocusFlow, please do not open a public GitHub issue.

Instead, report it privately using one of the following methods:

1. Use GitHub's **Report a vulnerability** option from the repository Security tab, if available.
2. If private vulnerability reporting is unavailable, contact the maintainer directly.

Please include:

* A clear description of the issue
* Steps to reproduce
* Affected version
* Screenshots, logs, or proof-of-concept details if applicable
* Any suggested fix, if you have one

## Scope

Security issues may include, but are not limited to:

* Unsafe file or path handling
* Arbitrary command execution
* Vulnerabilities related to the bundled FFmpeg sidecar
* Unauthorized access to recorded files
* Incorrect handling of user-generated recordings
* Tauri permission or capability misconfiguration

## Response Timeline

I will try to acknowledge valid security reports within 72 hours.

If the report is accepted, I will work on a fix and publish an update as soon as reasonably possible. If the issue is declined, I will explain why it is not considered a security vulnerability.

## Disclosure

Please allow time for the issue to be fixed before publicly disclosing the vulnerability.

Thank you for helping keep FocusFlow secure.
