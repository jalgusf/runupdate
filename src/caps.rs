//! Linux capability handling.
//!
//! `runupdate` drives `snap` and `apt`/`dpkg`. Those tools do **not** honour
//! Linux capabilities: `dpkg` refuses any modifying operation unless the
//! effective UID is 0 ("requested operation requires superuser privilege"), and
//! `snap` talks to the `snapd` daemon, which authorises the *caller's UID* via
//! polkit (prompting for a password otherwise). Handing capabilities to those
//! child processes therefore does not help — they need a real root UID.
//!
//! So instead of trying to run the commands with a narrow capability set, the
//! `setup` subcommand grants this binary just `CAP_SETUID` and `CAP_SETGID`
//! (via `setcap`). At run time the tool uses those to switch its UID/GID to 0
//! (see [`become_root`]), and then runs the update commands as real root, which
//! is exactly what `apt`/`dpkg`/`snap` require.
//!
//! Granting `CAP_SETUID` is effectively granting the ability to become root, so
//! this is deliberately a small, honest capability set rather than a long list
//! that would give a false impression of fine-grained confinement.

use std::os::raw::{c_int, c_ulong};
use std::ptr;

/// The capabilities `setup` grants to this binary, as (name, number) pairs.
///
/// The names match `setcap(8)` syntax and the numbers match
/// `<linux/capability.h>`. Only two are needed:
///
/// * `cap_setuid` – switch the real/effective/saved UID to 0 (become root).
/// * `cap_setgid` – switch the GID and drop supplementary groups to match.
///
/// With a UID of 0 the spawned `apt`/`dpkg`/`snap` processes run as real root
/// and thus have every privilege they need; no further capabilities are
/// required.
pub const REQUIRED_CAPS: &[(&str, c_int)] = &[("cap_setgid", 6), ("cap_setuid", 7)];

// prctl(2) constant for reading the bounding set.
const PR_CAPBSET_READ: c_int = 23;

extern "C" {
    fn prctl(option: c_int, arg2: c_ulong, arg3: c_ulong, arg4: c_ulong, arg5: c_ulong) -> c_int;
    fn geteuid() -> u32;
    fn getuid() -> u32;
    fn getgid() -> u32;
    fn setuid(uid: u32) -> c_int;
    fn setgid(gid: u32) -> c_int;
    fn setgroups(size: usize, list: *const u32) -> c_int;
}

/// Returns true if the process is running as (effective) root.
pub fn is_root() -> bool {
    // Safe: `geteuid` has no preconditions and cannot fail.
    unsafe { geteuid() == 0 }
}

/// The process's real UID and GID.
pub fn real_ids() -> (u32, u32) {
    // Safe: `getuid`/`getgid` have no preconditions and cannot fail.
    unsafe { (getuid(), getgid()) }
}

/// Whether a capability is present in this process's bounding set.
///
/// The kernel refuses (with `EPERM`) to `execve` a file whose permitted
/// capabilities are not a subset of the caller's bounding set. In restricted
/// environments (many containers) some capabilities are masked out, so granting
/// them via `setcap` would make the binary unexecutable. We therefore only ever
/// grant capabilities that are available here.
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

/// The comma-separated list of all required capabilities in `setcap`/`getcap`
/// syntax, e.g. `cap_setgid,cap_setuid`.
pub fn cap_names() -> String {
    REQUIRED_CAPS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(",")
}

/// The `setcap` capability specification granting every *available* required
/// capability in the effective, permitted and inheritable sets (`+eip`).
///
/// `e` (effective) makes the capabilities active for the binary immediately, so
/// it can call `setuid(2)`/`setgid(2)` without any further ceremony. Returns
/// `None` if no required capability is available at all.
pub fn setcap_spec() -> Option<String> {
    let names: Vec<&str> = available_caps().into_iter().map(|(n, _)| n).collect();
    if names.is_empty() {
        None
    } else {
        Some(format!("{}+eip", names.join(",")))
    }
}

/// Attempt to become real root (UID/GID 0) using the granted `CAP_SETUID` /
/// `CAP_SETGID` capabilities, so that the `apt`/`dpkg`/`snap` child processes
/// run as root.
///
/// This is the mechanism that actually lets the update commands run without
/// `sudo`: after `setup` the binary carries `CAP_SETUID`/`CAP_SETGID` in its
/// effective set, so `setgid(0)`/`setuid(0)` succeed and switch the real,
/// effective and saved IDs to 0.
///
/// Best-effort and idempotent: returns `true` if the process ends up as
/// effective root (including when it already was), and `false` if elevation was
/// not possible — for example when the binary has no file capabilities and is
/// not already running under `sudo`. Order matters: supplementary groups and
/// the GID are handled before the UID, because dropping `CAP_SETUID` privilege
/// by switching UID first could prevent the GID change.
pub fn become_root() -> bool {
    if is_root() {
        return true;
    }

    // Safe: these calls only change this process's credentials; the pointer
    // passed to `setgroups` is null with length 0, which clears the set.
    unsafe {
        // Drop supplementary groups and set GID 0. Failures here are not fatal
        // (they need CAP_SETGID); the UID change below is what matters most.
        setgroups(0, ptr::null());
        setgid(0);
        setuid(0);
    }

    is_root()
}
