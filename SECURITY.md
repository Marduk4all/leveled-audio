# Security policy

## Supported versions

Security fixes are made for the latest release only. Please update to the [latest release](https://github.com/Marduk4all/leveled-audio/releases/latest) before reporting.

## Reporting a vulnerability

Please **do not open a public issue** for security problems.

Report them privately via GitHub: **Security → Report a vulnerability** in this repository ([direct link](https://github.com/Marduk4all/leveled-audio/security/advisories/new)).

Helpful information:

- **Version and operating system.**
- **Affected component:** app, command line, a crate, or the release workflow.
- **Steps or a file to reproduce.** For example, a malformed WAV file that crashes the program or makes it write outside the output folder.

You'll get an answer as soon as possible. Once a fix is released, the advisory will be published with credit to you, if you want.

## Scope

Leveled Audio works offline and opens only the files you give it. Relevant issues include crashes or memory problems from crafted WAV/RF64 files, writing to unexpected paths, overwriting input files, and problems in the release binaries or build pipeline.
