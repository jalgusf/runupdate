# runupdate

A small Rust tool that runs the usual Debian/Ubuntu system-update commands and,
optionally, lets you run them **without `sudo`** by granting the binary the
Linux capabilities the package managers actually need.

## What it does

```
snap refresh
apt update
apt upgrade -y
apt autoremove -y
```

It runs these in order, streams their output live, and prints a summary,
exiting non-zero if any required command failed. `snap` is skipped (not treated
as an error) when it is not installed.

## Usage

```
runupdate              Run the update commands and report their output.
runupdate setup        Grant this binary the required capabilities (needs root).
runupdate teardown     Remove those capabilities again (needs root).
runupdate --help       Show help.
runupdate --version    Show the version.
```

## The capability modes

Package managers require a real root UID — and, importantly, **they will not
accept mere capabilities instead**:

* `dpkg` refuses any system-modifying operation unless the effective UID is 0
  ("requested operation requires superuser privilege"), regardless of the
  capabilities the process holds.
* `snap` talks to the `snapd` daemon, which authorises the *caller's UID* via
  polkit — a non-root caller gets a password prompt (or `error: cancelled`).

So handing capabilities to the child processes does not help. Instead,
`runupdate setup` uses [`setcap(8)`] to grant the binary just `CAP_SETUID` and
`CAP_SETGID`. At run time the tool uses those to switch its own UID/GID to 0
(become real root) and then runs the update commands as root — which is exactly
what they require:

```console
$ sudo runupdate setup     # once, as root
$ runupdate                # thereafter, no sudo needed
```

`runupdate teardown` removes the capabilities again (`setcap -r`).

Both `setup` and `teardown` require root, because changing a file's
capabilities needs `CAP_SETFCAP`.

### Restricting who can use it

Since the capability grant is a path to root, `setup` also locks the binary
down to the user who ran it: it `chown`s the file to that user (from `SUDO_UID`,
or root when not run via sudo) and sets its mode to `0700`. So only that user
can execute it — anyone else must go through sudo:

```console
$ sudo runupdate setup      # run as alice via sudo
$ runupdate                 # alice: works, no sudo
$ # bob: "Permission denied"
```

(The `chown`/`chmod` happen before `setcap`, because changing a file's owner or
mode clears its capability xattr.)

### Capabilities granted

`cap_setuid`, `cap_setgid`.

That is deliberately minimal. `CAP_SETUID` is enough to become root, so a
binary that has been through `setup` should be treated as privileged — the
owning user can obtain root through it. Run `teardown` to revoke the grant when
you no longer need passwordless updates.

### Environment awareness

The kernel refuses to execute a file whose permitted capabilities are not a
subset of the current **bounding set** (common in containers). `setup`
therefore only grants capabilities that are present in the bounding set and
warns about any it skips, so it can never leave the binary unexecutable.

## Building

```console
$ cargo build --release
# binary at target/release/runupdate
```

The tool has **no third-party dependencies** — it talks to the kernel via a few
libc calls (`prctl`, `setuid`/`setgid`) and shells out to the system `setcap`
for the privileged parts (install it with `apt install libcap2-bin` if
missing).

[`setcap(8)`]: https://man7.org/linux/man-pages/man8/setcap.8.html
