# Security Policy

## 1. Security Commitment

ENUT by EZTYN is an open-source AI usage monitoring application. We value the security and privacy of our users and welcome good-faith reports from security researchers and the open-source community.

## 2. Supported Versions

| Version | Security support |
| --- | --- |
| Latest stable release | Supported |
| Default development branch | Best effort |
| Older releases | Not guaranteed |

Users should upgrade to the latest stable release whenever possible.

## 3. Reporting a Vulnerability

**Do not disclose security vulnerabilities through public issues, pull requests, discussions, or social media before coordinated disclosure.**

To report privately:

1. Open the official ENUT GitHub repository.
2. Select the **Security** tab.
3. Open **Advisories** and select **Report a vulnerability**.
4. Submit your findings using the template below.

If private reporting is unavailable, open a public issue requesting a private security contact without including vulnerability details.

For ordinary bugs and feature requests without security implications, use GitHub Issues.

### Report template

```text
Title:
Summary:
Affected version / commit SHA:
Operating system and architecture:
Affected component:
Prerequisites:
Steps to reproduce:
Expected behavior:
Actual behavior:
Security impact:
Proof of concept (sanitized, if available):
CWE identifier (optional):
CVSS v4.0 score and vector (optional):
Suggested mitigation (optional):
Disclosure or attribution preferences (optional):
```

Never include live API keys, OAuth tokens, session cookies, passwords, unredacted usage logs, personal information, or other users' data. Use synthetic data and test credentials.

## 4. Scope

### In scope

Security defects affecting official ENUT source code, releases, dependencies as used by ENUT, and project-controlled build and distribution workflows, including:

- Unauthorized access to credentials, usage records, or local files.
- Exposure of API keys, OAuth tokens, session information, or sensitive logs.
- Command injection, arbitrary code execution, path traversal, and unsafe file handling.
- Authentication or authorization bypass, where applicable.
- Insecure IPC, local APIs, or network communication, where applicable.
- Insecure credential storage or transmission.
- Malicious update delivery, release tampering, and software supply-chain compromise.
- Dependency vulnerabilities with a demonstrable impact on ENUT.
- Privacy violations and unintended transmission of sensitive user data.

### Out of scope

- Third-party AI providers' infrastructure, accounts, and services.
- Unofficial forks and independently operated deployments without owner authorization.
- Social engineering, phishing, physical attacks, spam, and denial-of-service testing against shared systems.
- Automated scanner findings without a plausible security impact, version-only findings, and ordinary functional bugs. Additional evidence may bring these findings into scope.

Third-party vulnerabilities should also be reported to the affected vendor through its own disclosure process. This policy does not authorize testing third-party systems.

## 5. Responsible Security Research and Safe Harbor

Research must be conducted in good faith and limited to systems, accounts, and data you own or are explicitly authorized to test.

- Use isolated local environments and synthetic data whenever possible.
- Minimize access to data and stop once the vulnerability has been demonstrated.
- Do not access, modify, retain, or transmit other users' information.
- Do not establish persistence, disrupt services, or compromise release infrastructure.
- If sensitive information is encountered accidentally, stop testing and report the incident privately without sharing the sensitive values.
- Coordinate disclosure with maintainers and comply with applicable law.

To the extent within our authority, ENUT maintainers consider research that complies with this policy authorized and do not intend to initiate legal action solely for such research. This commitment does not grant immunity from applicable law or bind third parties.

## 6. Severity Classification

We use [CVSS v4.0](https://www.first.org/cvss/v4.0/specification-document) as a baseline for technical severity. Remediation priority also considers exploitability, active exploitation, user exposure, privacy impact, and available mitigations.

| Severity | CVSS score | Target initial triage | Target remediation |
| --- | --- | --- | --- |
| Critical | 9.0–10.0 | 48 hours | 7 days |
| High | 7.0–8.9 | 5 days | 30 days |
| Medium | 4.0–6.9 | 10 days | 60 days |
| Low | 0.1–3.9 | 15 days | 90 days |
| None | 0.0 | As capacity permits | Not applicable |

These are best-effort targets measured in calendar days, not contractual service-level agreements. Complex issues or dependency coordination may require additional time. Active exploitation or credential exposure may warrant accelerated mitigation regardless of CVSS score.

## 7. Vulnerability Handling Process

1. Acknowledge receipt, aiming for 48 hours.
2. Validate the finding and determine affected versions, severity, and scope.
3. Coordinate privately with the reporter and relevant upstream maintainers.
4. Develop and test a fix or mitigation, including regression tests where appropriate.
5. Publish a patched release and provide upgrade instructions.
6. Publish a GitHub Security Advisory where appropriate and request a CVE identifier if eligible.
7. Credit the reporter with their consent and close the report once appropriate follow-up is complete.

We aim to provide status updates at least every 14 days while a confirmed vulnerability remains unresolved.

## 8. Coordinated Disclosure

We request an initial coordination period of up to 90 calendar days from receipt of the report, or a mutually agreed alternative. Earlier disclosure may be appropriate when users need urgent protection, exploitation is occurring, or a fix is already available.

If a report cannot be resolved within the coordination period, maintainers and researchers should discuss mitigations, extensions, and a disclosure date. We do not require indefinite confidentiality.

Public advisories should describe affected versions, impact, severity, fixed versions, mitigations, and attribution where agreed, without exposing private user information or live secrets.

## 9. Privacy and Data Handling

Security reports are handled privately and shared only with individuals necessary to investigate and remediate the issue. We may coordinate sanitized technical details with affected upstream maintainers or vulnerability coordination organizations.

Please provide only the minimum information necessary to reproduce a finding. Do not send credential files, full home-directory archives, or unredacted AI conversation histories.

## 10. Recognition and Rewards

We appreciate responsible security research and may acknowledge contributors in security advisories and release notes with their permission. Anonymous reporting attribution is respected where possible.

ENUT does not currently offer a paid bug bounty under this policy. No financial reward is implied by submitting a report.

## 11. References

- [GitHub Private Vulnerability Reporting](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/report-privately)
- [OpenSSF Coordinated Vulnerability Disclosure Guide](https://oss-vulnerability-guide.openssf.org/maintainer-guide.html)
- [OWASP Vulnerability Disclosure Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Vulnerability_Disclosure_Cheat_Sheet.html)
- [FIRST CVSS v4.0](https://www.first.org/cvss/v4.0/specification-document)
- [MITRE Common Weakness Enumeration](https://cwe.mitre.org/)

Thank you for helping make ENUT and its open-source community more secure.

**ENUT — EZTYN**
