# rush

`rush` is a shell, mostly aimed at interactive use. It is highly inspired by
[`fish`](https://fishshell.com/), but with fewer features.

## Goals

Before `rush` can be considered feature complete, the following features are
required:

## Repository statistics

![Commits since latest version badge](https://img.shields.io/github/commits-since/lupont/rush/latest?style=flat-square)
![License badge](https://img.shields.io/github/license/lupont/rush?style=flat-square)
![Most used language badge](https://img.shields.io/github/languages/top/lupont/rush?style=flat-square)

### Tab completion

Tab completion that works on files, as well as arguments to programs. It will
probably be quite simple, maybe parsing man pages (like fish) will be
considered.

A simple builtin for generating completion files would also be great.

### Sophisticated input parsing

Currently, the input parsing always separates on space characters, returning the
first entry as the command and the following as arguments. There are a few ways
in which this must be improved.

#### Globbing and expansion

`*` and `**` to expand to everything, non-recursively and recursively,
respectively. Environment variable expansion by `$`. Skipping separation by
wrapping multiple words in either single quotes or double quotes, the latter
expanding variables within.

#### Piping

Laying down pipes by separating statements with `|`, which makes the process
redirect its stdout to the following process's stdin.

#### Redirections

Redirecting the stdout (and stderr) streams with `>`, `>>`, `2>`, etc.

### Environment variables

Part of this ties in with the [globbing and expansion](#globbing-and-expansion)
section, but to support environment variables fully one should be able to set
them on a process basis (i.e. `FOO=BAR ls`), and exporting them to the current
session.

### Configurability

The shell should be able to execute arbitrary files, reading one (or some) of
them at login. Most likely in `$XDG_CONFIG_HOME/rush/init.rush` or similar.

### More advanced CLI

The rush CLI should be able to accept some flags, in part in order to be POSIX
compliant (I believe). Things like `-c` to execute something, and probably a
bunch more.

### More builtins

The shell needs to support looping with `for`, `while`, if statements with `if [
<stmt> ]; then; else; fi`, and a bunch more for POSIX compliance (and more
ergonomic day to day use).

## Current state

The shell is in heavy development and is quite unstable at the moment. The
features described in [goals](#goals) are mostly lacking currently. The
following is a high-level checklist of what is in and what is not.

- [ ] Tab completion
- [ ] Sophisticated input parsing
- [ ] Globbing and expansion
    - [x] Expand `~` to (expanded) `$HOME`
    - [ ] Piping
    - [ ] Redirections
- [ ] Environment variables
- [ ] Configurability
- [ ] More advanced CLI
- [ ] Expected builtins

## Contributing

`rush` is written in Rust, and uses `cargo` for building, linting, formatting,
and testing. Contributing requires having the stable Rust toolchain installed.
The recommended way to install this is with [rustup](https://rustup.rs).
