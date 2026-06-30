# Security Policy

`wickra-exchange` connects to crypto exchanges and can place authenticated
orders. It handles **secret key material** and **order flow**, so security is a
first-class concern. Please read [THREAT_MODEL.md](THREAT_MODEL.md) for the asset
inventory, trust boundaries and key-handling guarantees.

## Supported versions

This project is pre-release. Security fixes target the `main` branch and the most
recent published version once a release exists.

| Version | Supported |
|---------|-----------|
| `main`  | ✅        |
| `0.1.x` (upcoming) | ✅ |

## Reporting a vulnerability

**Please do not open a public issue, pull request or discussion for security
problems.** Report privately through either channel:

- GitHub → the repository's **Security** tab → **Report a vulnerability**
  (private advisory), or
- email **support@wickra.org**.

Include a description, affected version/commit, reproduction steps and impact.
**Never include real API keys, secrets or signed request payloads** — redact them.

We aim to acknowledge within a few days, agree a disclosure timeline, and credit
reporters who wish to be named once a fix ships.

## Scope

In scope: signing correctness, leakage of secret material (logs, errors, memory),
order-rounding/validation flaws that could mis-size or mis-route an order, and
reconnect/reconciliation gaps that could lose track of open orders. Out of scope:
vulnerabilities in third-party exchanges themselves, and any use that puts secret
keys in a browser or other untrusted client (explicitly unsupported — see the
threat model).
