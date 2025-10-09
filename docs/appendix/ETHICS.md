# Ethics & Safety Appendix

## Overview

This project is designed for **defensive security research** and **EDR evaluation** in controlled lab environments. All work must adhere to strict ethical guidelines.

## Core Principles

### 1. Lab-Only Policy

**All experiments MUST be conducted in isolated lab environments:**

- ✅ Air-gapped networks or strictly firewalled segments
- ✅ Dedicated test VMs with snapshots
- ✅ No production infrastructure
- ✅ No public-facing systems
- ✅ No connection to corporate networks

**Prohibited:**
- ❌ Running on personal machines with sensitive data
- ❌ Execution on production servers
- ❌ Testing against third-party systems without written authorization
- ❌ Public cloud environments without proper isolation

### 2. Defensive Focus

**This project simulates malicious behavior for blue team purposes:**

- ✅ Understanding EDR detection mechanisms
- ✅ Improving defensive capabilities
- ✅ Training security teams
- ✅ Validating detection rules

**NOT for:**
- ❌ Creating operational malware
- ❌ Bypassing security controls maliciously
- ❌ Unauthorized access to systems

### 3. No Operational Payloads

Per CLAUDE.md Section 0:
> "This project does not use operational payload but tries to imitate its behavior so it can be used in a safe manner."

**Implementation:**
- All "malicious" samples use benign code that mimics patterns
- No actual exploitation code
- No credential theft
- No data exfiltration
- No lateral movement
- Focus on **evasion techniques**, not **malicious outcomes**

## Data Handling

### Data Sanitization

Before external release or sharing:

1. **Remove PII:**
   - Usernames
   - Machine names
   - Domain names
   - Email addresses

2. **Redact Infrastructure:**
   - Internal IP addresses
   - File paths containing org names
   - Custom registry paths

3. **Anonymize Artifacts:**
   - Replace real malware hashes with placeholders
   - Remove organization-specific IOCs

### Data Retention

- Telemetry: 90 days (configurable ILM)
- Artifacts: Hash-based deduplication
- Experiment logs: Indefinite with sanitization

## Responsible Disclosure

### If Security Issues Are Discovered

1. **Vulnerability in Vendor Product:**
   - Report via vendor's security contact
   - Follow coordinated disclosure timeline (typically 90 days)
   - Do not publish proof-of-concept until coordinated

2. **EDR Bypass Technique:**
   - Notify affected EDR vendor first
   - Allow reasonable remediation time
   - Publish defensively-focused write-up (not exploit code)

3. **Project Security Issue:**
   - Report via GitHub Security Advisories
   - Do not weaponize findings

## Collaboration Guidelines

### Academic Research
- Cite this work appropriately
- Share sanitized datasets when possible
- Coordinate with authors on publications

### Industry Use
- This project is open source for defensive use
- Commercial use: Review licensing
- Do not rebrand as offensive tooling

## Legal Compliance

### Jurisdictional Considerations

**Users are responsible for compliance with:**
- Local computer fraud laws (e.g., CFAA in USA)
- Export control regulations
- Industry-specific regulations (HIPAA, PCI-DSS, etc.)

### Authorization

Before running experiments:
- Obtain written authorization from system owners
- Document scope and timeline
- Maintain audit logs

## Prohibited Uses

This project **MUST NOT** be used for:

1. Unauthorized access to computer systems
2. Creating malware for distribution
3. Violating terms of service of cloud providers
4. Circumventing security for malicious purposes
5. Academic dishonesty or plagiarism
6. Harassment or stalking

## Reporting Misuse

If you observe misuse of this project:
- **Report to:** [maintainer email]
- **Include:** Description, evidence, impact
- **Response time:** Within 48 hours

## Acknowledgment

By using this project, you acknowledge:
- Understanding of these ethical guidelines
- Agreement to use defensively and in authorized environments
- Responsibility for compliance with applicable laws
- Commitment to responsible disclosure

---

**Last Updated:** [Date]
**Version:** 1.0
