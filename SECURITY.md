# Security policy

## Supported versions

Security fixes are provided for the latest released version of `lazy`. Upgrade
to the newest release before reporting an issue that may already be fixed.

## Reporting a vulnerability

Please do not open a public issue. Use GitHub's private vulnerability reporting
form:

https://github.com/ineentho/lazy/security/advisories/new

Include the affected version and platform, reproduction steps, impact, and any
suggested mitigation. You should receive an acknowledgement within seven days.
Please allow time for a fix and coordinated release before public disclosure.

## Scope

Reports about escaping the documented loopback and same-user trust boundaries,
unauthorized access through the control socket, request-routing confusion, TLS
handling, or unintended command execution are in scope.

`lazy` is intentionally an unauthenticated development proxy. Access by a
client that the user deliberately admitted to a non-loopback listener is not by
itself a vulnerability. See the README's security model for deployment limits.
