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

Package managers normally require full root. Instead of always using `sudo`,
`runupdate setup` uses [`setcap(8)`] to attach a fixed set of file capabilities
to the binary (in the permitted + inheritable + effective sets). At run time the
tool raises those capabilities into its **ambient** set so they survive `exec`
and are inherited by the `snap`/`apt` child processes. The result is that an
otherwise unprivileged user can run updates:

```console
$ sudo runupdate setup     # once, as root
$ runupdate                # thereafter, no sudo needed
```

`runupdate teardown` removes the capabilities again (`setcap -r`).

Both `setup` and `teardown` require root, because changing a file's
capabilities needs `CAP_SETFCAP`.

### Environment awareness

The kernel refuses to execute a file whose permitted capabilities are not a
subset of the current **bounding set** (common in containers). `setup`
therefore only grants capabilities that are present in the bounding set and
warns about any it skips, so it can never leave the binary unexecutable.

### Capabilities granted

`cap_chown`, `cap_dac_override`, `cap_dac_read_search`, `cap_fowner`,
`cap_fsetid`, `cap_kill`, `cap_setgid`, `cap_setuid`, `cap_setpcap`,
`cap_net_bind_service`, `cap_net_admin`, `cap_net_raw`, `cap_sys_chroot`,
`cap_sys_ptrace`, `cap_sys_admin`, `cap_sys_resource`, `cap_mknod`,
`cap_audit_write`, `cap_setfcap`.

This set is broad by necessity — `apt`/`dpkg` run arbitrary maintainer scripts
and `snap` mounts squashfs images. Run `teardown` to revoke it when you no
longer need passwordless updates.

## Building

```console
$ cargo build --release
# binary at target/release/runupdate
```

The tool has **no third-party dependencies** — it talks to the kernel via a
couple of `prctl(2)` calls and shells out to the system `setcap` for the
privileged parts (install it with `apt install libcap2-bin` if missing).

[`setcap(8)`]: https://man7.org/linux/man-pages/man8/setcap.8.html
