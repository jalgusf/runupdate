//! Linux capability handling.
//!
//! `runupdate` drives `snap` and `apt`/`dpkg`, which normally require full
//! root. Rather than always running under `sudo`, the `setup` subcommand grants
//! this binary a fixed set of file capabilities (via `setcap`). At run time we
//! then raise those capabilities into the *ambient* set so that they survive
//! `execve(2)` and are inherited by the `snap`/`apt` child processes.
//!
//! For an ambient capability to be raisable it must be present in both the
//! process's permitted and inheritable sets. That is exactly what
//! `setcap "<caps>+eip"` arranges when the binary is executed, so the two
//! halves (`setup` + ambient raise) are designed to work together.

use std::os::raw::{c_int, c_ulong};

/// The capabilities the update commands need, as (name, number) pairs.
///
/// The names match `setcap(8)` syntax (e.g. `cap_sys_admin`) and the numbers
/// match `<linux/capability.h>`. This is the single source of truth used both
/// to build the `setcap` argument and to raise the ambient set at run time.
///
/// The set is deliberately broad because `apt`/`dpkg` run arbitrary maintainer
/// scripts and `snap` mounts squashfs images:
///
/// * `cap_chown`, `cap_fowner`, `cap_fsetid` – adjust ownership / mode bits and
///   preserve set-id bits while unpacking packages.
/// * `cap_dac_override`, `cap_dac_read_search` – read/write/traverse the dpkg
///   database, caches and system files regardless of their permissions.
/// * `cap_setuid`, `cap_setgid`, `cap_setpcap` – dpkg drops privileges when
///   running maintainer scripts and adjusts capabilities.
/// * `cap_setfcap` – install binaries that themselves carry file capabilities
///   (e.g. `ping`).
/// * `cap_mknod` – create device nodes shipped in packages.
/// * `cap_kill` – signal running services during upgrades.
/// * `cap_net_bind_service`, `cap_net_admin`, `cap_net_raw` – configure the
///   network and (re)start privileged-port services.
/// * `cap_sys_admin` – mount squashfs images for snaps and other mount/namespace
///   operations performed by snapd.
/// * `cap_sys_chroot`, `cap_sys_ptrace`, `cap_sys_resource`, `cap_audit_write`
///   – assorted operations performed by package scripts and snapd.
pub const REQUIRED_CAPS: &[(&str, c_int)] = &[
    ("cap_chown", 0),
    ("cap_dac_override", 1),
    ("cap_dac_read_search", 2),
    ("cap_fowner", 3),
    ("cap_fsetid", 4),
    ("cap_kill", 5),
    ("cap_setgid", 6),
    ("cap_setuid", 7),
    ("cap_setpcap", 8),
    ("cap_net_bind_service", 10),
    ("cap_net_admin", 12),
    ("cap_net_raw", 13),
    ("cap_sys_chroot", 18),
    ("cap_sys_ptrace", 19),
    ("cap_sys_admin", 21),
    ("cap_sys_resource", 24),
    ("cap_mknod", 27),
    ("cap_audit_write", 29),
    ("cap_setfcap", 31),
];

// prctl(2) constants.
const PR_CAPBSET_READ: c_int = 23;
const PR_CAP_AMBIENT: c_int = 47;
const PR_CAP_AMBIENT_RAISE: c_ulong = 2;
const PR_CAP_AMBIENT_CLEAR_ALL: c_ulong = 4;

extern "C" {
    fn prctl(option: c_int, arg2: c_ulong, arg3: c_ulong, arg4: c_ulong, arg5: c_ulong) -> c_int;
    fn geteuid() -> u32;
}

/// Returns true if the process is running as (effective) root.
pub fn is_root() -> bool {
    // Safe: `geteuid` has no preconditions and cannot fail.
    unsafe { geteuid() == 0 }
}

/// Whether a capability is present in this process's bounding set.
///
/// This matters because the kernel refuses (with `EPERM`) to `execve` a file
/// whose permitted capabilities are not a subset of the caller's bounding set.
/// In restricted environments (many containers) some capabilities are masked
/// out of the bounding set, so granting them via `setcap` would make the binary
/// unexecutable. We therefore only ever grant capabilities that are available
/// here.
fn in_bounding_set(cap: c_int) -> bool {
    // Safe: PR_CAPBSET_READ only reads the bounding set; returns 1/0/-1.
    unsafe { prctl(PR_CAPBSET_READ, cap as c_ulong, 0, 0, 0) == 1 }
}

/// The subset of [`REQUIRED_CAPS`] that is actually available in the current
/// bounding set (and can therefore be granted and used safely).
pub fn available_caps() -> Vec<(&'static str, c_int)> {
    REQUIRED_CAPS
        .iter()
        .copied()
        .filter(|(_, num)| in_bounding_set(*num))
        .collect()
}

/// Required capabilities that are *not* available in the current bounding set
/// and will be skipped by `setup`.
pub fn unavailable_caps() -> Vec<&'static str> {
    REQUIRED_CAPS
        .iter()
        .filter(|(_, num)| !in_bounding_set(*num))
        .map(|(name, _)| *name)
        .collect()
}

/// The comma-separated list of *all* required capabilities in
/// `setcap`/`getcap` syntax, e.g. `cap_chown,cap_dac_override,...`. Used for
/// documentation (`--help`); the actually-granted set may be smaller — see
/// [`available_caps`].
pub fn cap_names() -> String {
    REQUIRED_CAPS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(",")
}

/// The `setcap` capability specification granting every *available* required
/// capability in the effective, inheritable and permitted sets (`+eip`).
///
/// `p` (permitted) + `i` (inheritable) are what allow the ambient set to be
/// raised later; `e` (effective) makes the capabilities active for the binary
/// itself. Returns `None` if no required capability is available at all.
pub fn setcap_spec() -> Option<String> {
    let names: Vec<&str> = available_caps().into_iter().map(|(n, _)| n).collect();
    if names.is_empty() {
        None
    } else {
        Some(format!("{}+eip", names.join(",")))
    }
}

/// Attempt to raise every required capability into the ambient set so that the
/// `snap`/`apt` child processes inherit them across `execve`.
///
/// This is best-effort: when the binary carries no file capabilities (for
/// example when the whole tool is simply run under `sudo`) the raise fails with
/// `EPERM` and is ignored, because in that case the children already inherit
/// full privileges from the root parent. Returns the number of capabilities
/// successfully raised.
pub fn raise_ambient() -> usize {
    let mut raised = 0;
    for (_, num) in available_caps() {
        // Safe: PR_CAP_AMBIENT_RAISE only reads `arg3` (the cap number) and has
        // no memory-safety implications.
        let rc = unsafe {
            prctl(
                PR_CAP_AMBIENT,
                PR_CAP_AMBIENT_RAISE,
                num as c_ulong,
                0,
                0,
            )
        };
        if rc == 0 {
            raised += 1;
        }
    }
    raised
}

/// Clear the entire ambient capability set for this process. Best-effort.
pub fn clear_ambient() {
    // Safe: no memory is touched.
    unsafe {
        prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_CLEAR_ALL, 0, 0, 0);
    }
}
