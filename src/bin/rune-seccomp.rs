//! rune-seccomp: applies a seccomp BPF filter then exec's the remaining args.
//! Blocks: ptrace, mount, unshare, kexec_load, bpf, setns
//! Allows everything else.

use std::env;
use std::os::unix::process::CommandExt;
use std::process::Command;

// seccomp_data offsets
const OFFSET_NR: u32 = 0;       // offsetof(seccomp_data, nr)
const OFFSET_ARCH: u32 = 4;     // offsetof(seccomp_data, arch)

// Audit arch for x86_64
const AUDIT_ARCH_X86_64: u32 = 0xC000003E;

// Seccomp
const SECCOMP_RET_ALLOW: u32 = 0x7FFF0000;
const SECCOMP_RET_KILL_PROCESS: u32 = 0x80000000;
const SECCOMP_RET_ERRNO: u32 = 0x00050000;
const EPERM: u32 = 1;

// BPF
const BPF_LD: u16 = 0x00;
const BPF_W: u16 = 0x00;
const BPF_ABS: u16 = 0x20;
const BPF_JMP: u16 = 0x05;
const BPF_JEQ: u16 = 0x10;
const BPF_K: u16 = 0x00;
const BPF_RET: u16 = 0x06;

// x86_64 syscall numbers
const SYS_PTRACE: u32 = 101;
const SYS_MOUNT: u32 = 165;
const SYS_KEXEC_LOAD: u32 = 246;
const SYS_UNSHARE: u32 = 272;
const SYS_SETNS: u32 = 308;
const SYS_BPF: u32 = 321;

#[repr(C)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

fn bpf_stmt(code: u16, k: u32) -> SockFilter {
    SockFilter { code, jt: 0, jf: 0, k }
}

fn bpf_jump(code: u16, k: u32, jt: u8, jf: u8) -> SockFilter {
    SockFilter { code, jt, jf, k }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: rune-seccomp <command> [args...]");
        std::process::exit(1);
    }

    let blocked = [SYS_PTRACE, SYS_MOUNT, SYS_UNSHARE, SYS_KEXEC_LOAD, SYS_BPF, SYS_SETNS];
    let num_blocked = blocked.len();

    // Build BPF program:
    // 1. Check arch == x86_64, if not -> allow
    // 2. Load syscall nr
    // 3. For each blocked: if match -> deny
    // 4. Allow
    let mut filter: Vec<SockFilter> = Vec::new();

    // Load arch
    filter.push(bpf_stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_ARCH));
    // If arch != x86_64, skip to allow (jump over all checks)
    filter.push(bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH_X86_64, 0, (num_blocked + 2) as u8));

    // Load syscall number
    filter.push(bpf_stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_NR));

    // For each blocked syscall
    for (i, &nr) in blocked.iter().enumerate() {
        let jump_to_deny = (num_blocked - i) as u8; // distance to deny instruction
        filter.push(bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, nr, jump_to_deny, 0));
    }

    // Allow (fell through all checks)
    filter.push(bpf_stmt(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));
    // Deny (matched a blocked syscall)
    filter.push(bpf_stmt(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM));

    let prog = SockFprog {
        len: filter.len() as u16,
        filter: filter.as_ptr(),
    };

    unsafe {
        // Required: set no_new_privs
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            eprintln!("rune-seccomp: prctl(NO_NEW_PRIVS) failed: {}", std::io::Error::last_os_error());
            std::process::exit(1);
        }

        // Apply seccomp filter via prctl (more compatible than seccomp() syscall)
        let ret = libc::prctl(
            libc::PR_SET_SECCOMP,
            2, // SECCOMP_MODE_FILTER
            &prog as *const SockFprog as libc::c_ulong,
            0,
            0,
        );
        if ret != 0 {
            eprintln!("rune-seccomp: prctl(SET_SECCOMP) failed: {}", std::io::Error::last_os_error());
            std::process::exit(1);
        }
    }

    // Exec target command
    let err = Command::new(&args[1]).args(&args[2..]).exec();
    eprintln!("rune-seccomp: exec failed: {}", err);
    std::process::exit(1);
}
