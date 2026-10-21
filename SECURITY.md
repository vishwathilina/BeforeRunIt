# Security policy

## Reporting a vulnerability

Please report security issues privately through
[GitHub security advisories](https://github.com/beforerunit/beforerunit/security/advisories/new),
not in public issues. Expect a first answer within 7 days.

## Scope

BeforeRunIt runs inside your interactive shell, so these are treated as
security bugs:

- A command line that makes BeforeRunIt execute code, write files, or crash
  the shell (the bash builtin runs inside the bash process).
- Secrets leaking to the AI endpoint, such as tokens, passwords, keys or
  `Authorization` headers, despite redaction.
- Another local user reaching the daemon socket, the cache, or the stored API
  key.
- A rule or AI answer making a destructive command look safe in a way the
  merge rules should prevent, for example by lowering a hard rule.

## Design notes

- The daemon socket lives in `$XDG_RUNTIME_DIR` (or a private `0700`
  directory), is created with mode `0600`, and rejects peers with a different
  user id.
- The daemon writes to a terminal only when that terminal belongs to the same
  user and the shell is in the foreground.
- API keys live in the system keyring, or in a `0600` file when no keyring is
  available.
- Requests to the AI contain only the redacted command line, the folder name,
  git state, the root flag, and whether target files exist.
- BeforeRunIt is an assistant, not a sandbox. A KNOWN badge is not a guarantee
  that a command is safe.
