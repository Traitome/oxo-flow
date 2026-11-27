#!/usr/bin/env python3
"""Landlock + user-namespace composition probe for issue #1401.

Installs a Landlock ruleset in THIS process and then execs unshare in the
same process (exec preserves the pid, so a rule resolved for /proc/self/...
still names the inode unshare later writes). Run it as the UNPRIVILEGED
server uid under the AppArmor `userns,` profile:

    sudo aa-exec -p oxo-flow-userns -- sudo -u ubuntu python3 -I \
        docs/superpowers/audits/data/rb01-userns-landlock-probe.py <variant>

Variants (the four measured in the 2026-11-27 audit):

  none         - no Landlock (control; must exit 0)
  system-only  - Landlock with the normal read/exec + write grants
  proc-grants  - + write rules on /proc/self/{setgroups,uid_map,gid_map}
  proc-self-dir- + a rule on the whole /proc/self directory

All three Landlock variants fail with EACCES opening /proc/self/uid_map on
the measured host (Landlock ABI 4, util-linux 2.39.3): with --fork util-linux
clones and the child writes its own dynamic /proc/<pid>/uid_map, which no
pre-exec static ruleset can name. Only a host-/proc grant would cover it,
re-exposing every process — so the modes are mutually exclusive (fail-closed).
"""
import ctypes
import os
import sys

SYS_CREATE, SYS_ADD, SYS_RESTRICT = 444, 445, 446  # x86_64 / aarch64
CREATE_VERSION, RULE_PATH_BENEATH = 1, 1
FS_EXECUTE, FS_WRITE_FILE, FS_READ_FILE, FS_READ_DIR = 1, 2, 4, 8
FS_REMOVE_DIR, FS_REMOVE_FILE, FS_MAKE_CHAR, FS_MAKE_DIR = 16, 32, 64, 128
FS_MAKE_REG, FS_MAKE_SOCK, FS_MAKE_FIFO, FS_MAKE_BLOCK = 256, 512, 1024, 2048
FS_MAKE_SYM = 4096
V1 = (
    FS_EXECUTE | FS_WRITE_FILE | FS_READ_FILE | FS_READ_DIR | FS_REMOVE_DIR
    | FS_REMOVE_FILE | FS_MAKE_CHAR | FS_MAKE_DIR | FS_MAKE_REG
    | FS_MAKE_SOCK | FS_MAKE_FIFO | FS_MAKE_BLOCK | FS_MAKE_SYM
)

libc = ctypes.CDLL(None, use_errno=True)
libc.syscall.restype = ctypes.c_long
libc.prctl.restype = ctypes.c_int


class RulesetAttr(ctypes.Structure):
    _fields_ = [("handled_access_fs", ctypes.c_uint64)]


class PathBeneathAttr(ctypes.Structure):
    _fields_ = [("allowed_access", ctypes.c_uint64), ("parent_fd", ctypes.c_int32)]


def note(msg):
    os.write(2, (msg + "\n").encode())


def abi():
    return libc.syscall(SYS_CREATE, None, 0, CREATE_VERSION)


def install(rules):
    attr = RulesetAttr(V1)
    rs = libc.syscall(SYS_CREATE, ctypes.byref(attr), ctypes.sizeof(attr), 0)
    if rs < 0:
        e = ctypes.get_errno()
        raise OSError(e, "create_ruleset: " + os.strerror(e))
    for path, access in rules:
        try:
            fd = os.open(path, os.O_PATH | os.O_CLOEXEC)
        except FileNotFoundError:
            note("skip missing grant path " + path)
            continue
        pa = PathBeneathAttr(access, fd)
        rc = libc.syscall(SYS_ADD, rs, RULE_PATH_BENEATH, ctypes.byref(pa), 0)
        os.close(fd)
        if rc < 0:
            e = ctypes.get_errno()
            raise OSError(e, "add_rule %s: %s" % (path, os.strerror(e)))
    if libc.prctl(38, 1, 0, 0, 0) != 0:  # PR_SET_NO_NEW_PRIVS
        e = ctypes.get_errno()
        raise OSError(e, "prctl: " + os.strerror(e))
    rc = libc.syscall(SYS_RESTRICT, rs, 0)
    os.close(rs)
    if rc < 0:
        e = ctypes.get_errno()
        raise OSError(e, "restrict_self: " + os.strerror(e))


def main():
    variant = sys.argv[1] if len(sys.argv) > 1 else "none"
    note("probe variant=%s abi=%s" % (variant, abi()))
    if variant != "none":
        rules = []
        for d in ["/usr", "/bin", "/sbin", "/lib", "/lib64", "/lib32"]:
            if os.path.isdir(d):
                rules.append((d, FS_EXECUTE | FS_READ_FILE | FS_READ_DIR))
        rules.append(("/tmp", V1))
        if variant == "proc-grants":
            rules.append(("/proc/self/setgroups", FS_WRITE_FILE))
            rules.append(("/proc/self/uid_map", FS_READ_FILE | FS_WRITE_FILE))
            rules.append(("/proc/self/gid_map", FS_READ_FILE | FS_WRITE_FILE))
        elif variant == "proc-self-dir":
            rules.append(("/proc/self", V1))
        elif variant != "system-only":
            note("unknown variant " + variant)
            sys.exit(2)
        try:
            install(rules)
        except OSError as e:
            note("LANDLOCK-INSTALL-FAILED: %s" % e)
            sys.exit(90)
        note("landlock ruleset installed")
    argv = [
        "unshare", "--user", "--map-root-user", "--pid", "--fork",
        "--mount-proc", "--kill-child=SIGTERM", "--", "true",
    ]
    try:
        os.execv("/usr/bin/unshare", argv)
    except OSError as e:
        note("EXEC-FAILED: %s" % e)
        sys.exit(91)


main()
