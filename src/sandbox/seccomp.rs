#![allow(dead_code, unused_imports, unused_variables)]
#![allow(clippy::all)]
//! rune _seccomp: applies a seccomp BPF filter then exec's the remaining args.
//! Blocks: ptrace, mount, unshare, kexec_load, bpf, setns
//! Optionally blocks network if --block-network is passed.

use std::env;
use std::os::unix::process::CommandExt;
use std::process::Command;

const OFFSET_NR: u32 = 0;
const OFFSET_ARCH: u32 = 4;
const AUDIT_ARCH_X86_64: u32 = 0xC000003E;
const SECCOMP_RET_ALLOW: u32 = 0x7FFF0000;
const SECCOMP_RET_ERRNO: u32 = 0x00050000;
const EPERM: u32 = 1;
const BPF_LD: u16 = 0x00;
const BPF_W: u16 = 0x00;
const BPF_ABS: u16 = 0x20;
const BPF_JMP: u16 = 0x05;
const BPF_JEQ: u16 = 0x10;
const BPF_K: u16 = 0x00;
const BPF_RET: u16 = 0x06;

const SYS_PTRACE: u32 = 101;
const SYS_MOUNT: u32 = 165;
const SYS_KEXEC_LOAD: u32 = 246;
const SYS_UNSHARE: u32 = 272;
const SYS_SETNS: u32 = 308;
const SYS_BPF: u32 = 321;
const SYS_SOCKET: u32 = 41;
const SYS_CONNECT: u32 = 42;
const SYS_ACCEPT: u32 = 43;
const SYS_SENDTO: u32 = 44;
const SYS_RECVFROM: u32 = 45;
const SYS_SENDMSG: u32 = 46;
const SYS_RECVMSG: u32 = 47;
const SYS_BIND: u32 = 49;
const SYS_LISTEN: u32 = 50;

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
    SockFilter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}
fn bpf_jump(code: u16, k: u32, jt: u8, jf: u8) -> SockFilter {
    SockFilter { code, jt, jf, k }
}

fn syscall_name_to_nr(name: &str) -> Option<u32> {
    match name.trim() {
        "ptrace" => Some(SYS_PTRACE),
        "mount" => Some(SYS_MOUNT),
        "unshare" => Some(SYS_UNSHARE),
        "kexec_load" => Some(SYS_KEXEC_LOAD),
        "bpf" => Some(SYS_BPF),
        "setns" => Some(SYS_SETNS),
        "socket" => Some(SYS_SOCKET),
        "connect" => Some(SYS_CONNECT),
        "accept" => Some(SYS_ACCEPT),
        "bind" => Some(SYS_BIND),
        "listen" => Some(SYS_LISTEN),
        _ => None,
    }
}

/// Entry point when invoked as `rune _landlock` / `_seccomp` / `_net-guard` subcommand.
pub fn run() {
    let all_args: Vec<String> = env::args().collect();
    let args: Vec<String> = all_args[1..].to_vec(); // skip binary name, keep subcommand as args[0]
    if args.len() < 2 {
        eprintln!(
            "Usage: rune _seccomp [--block-network] [--allow-syscalls <list>] <command> [args...]"
        );
        std::process::exit(1);
    }

    let mut block_net = false;
    let mut allowed_syscalls: Vec<String> = Vec::new();
    let mut cmd_idx = 1;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--block-network" => block_net = true,
            "--allow-syscalls" => {
                i += 1;
                if i < args.len() {
                    allowed_syscalls = args[i].split(',').map(|s| s.trim().to_string()).collect();
                }
            }
            _ => {
                cmd_idx = i;
                break;
            }
        }
        i += 1;
    }
    if cmd_idx >= args.len() {
        std::process::exit(1);
    }

    // Build blocked list: all dangerous syscalls EXCEPT those in allowed_syscalls
    let all_dangerous = vec![
        SYS_PTRACE,
        SYS_MOUNT,
        SYS_UNSHARE,
        SYS_KEXEC_LOAD,
        SYS_BPF,
        SYS_SETNS,
    ];

    // If allowed_syscalls contains "*", block nothing
    let wildcard = allowed_syscalls.iter().any(|s| s == "*");

    let mut blocked: Vec<u32> = if wildcard {
        Vec::new()
    } else {
        all_dangerous
            .into_iter()
            .filter(|&nr| {
                // Keep in blocked list only if NOT in allowed_syscalls
                !allowed_syscalls
                    .iter()
                    .any(|name| syscall_name_to_nr(name) == Some(nr))
            })
            .collect()
    };

    if block_net {
        blocked.push(SYS_SOCKET);
        blocked.push(SYS_CONNECT);
        blocked.push(SYS_ACCEPT);
        blocked.push(SYS_BIND);
        blocked.push(SYS_LISTEN);
    }
    let num_blocked = blocked.len();

    let mut filter: Vec<SockFilter> = Vec::new();
    filter.push(bpf_stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_ARCH));
    filter.push(bpf_jump(
        BPF_JMP | BPF_JEQ | BPF_K,
        AUDIT_ARCH_X86_64,
        0,
        (num_blocked + 2) as u8,
    ));
    filter.push(bpf_stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_NR));

    for (i, &nr) in blocked.iter().enumerate() {
        let jump_to_deny = (num_blocked - i) as u8;
        filter.push(bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, nr, jump_to_deny, 0));
    }

    filter.push(bpf_stmt(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));
    filter.push(bpf_stmt(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM));

    let prog = SockFprog {
        len: filter.len() as u16,
        filter: filter.as_ptr(),
    };

    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            eprintln!("rune _seccomp: prctl(NO_NEW_PRIVS) failed");
            std::process::exit(1);
        }
        let ret = libc::prctl(
            libc::PR_SET_SECCOMP,
            2,
            &prog as *const SockFprog as libc::c_ulong,
            0,
            0,
        );
        if ret != 0 {
            eprintln!("rune _seccomp: prctl(SET_SECCOMP) failed");
            std::process::exit(1);
        }
    }

    let err = Command::new(&args[cmd_idx])
        .args(&args[cmd_idx + 1..])
        .exec();
    eprintln!("rune _seccomp: exec failed: {}", err);
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── syscall_name_to_nr ──────────────────────────────────────────────────

    #[test]
    fn test_syscall_name_to_nr_known() {
        assert_eq!(syscall_name_to_nr("ptrace"), Some(SYS_PTRACE));
        assert_eq!(syscall_name_to_nr("mount"), Some(SYS_MOUNT));
        assert_eq!(syscall_name_to_nr("unshare"), Some(SYS_UNSHARE));
        assert_eq!(syscall_name_to_nr("kexec_load"), Some(SYS_KEXEC_LOAD));
        assert_eq!(syscall_name_to_nr("bpf"), Some(SYS_BPF));
        assert_eq!(syscall_name_to_nr("setns"), Some(SYS_SETNS));
        assert_eq!(syscall_name_to_nr("socket"), Some(SYS_SOCKET));
        assert_eq!(syscall_name_to_nr("connect"), Some(SYS_CONNECT));
        assert_eq!(syscall_name_to_nr("accept"), Some(SYS_ACCEPT));
        assert_eq!(syscall_name_to_nr("bind"), Some(SYS_BIND));
        assert_eq!(syscall_name_to_nr("listen"), Some(SYS_LISTEN));
    }

    #[test]
    fn test_syscall_name_to_nr_unknown() {
        assert_eq!(syscall_name_to_nr("foobar"), None);
        assert_eq!(syscall_name_to_nr(""), None);
        assert_eq!(syscall_name_to_nr("PTRACE"), None);
        assert_eq!(syscall_name_to_nr("execve"), None);
    }

    #[test]
    fn test_syscall_name_to_nr_whitespace_trimmed() {
        assert_eq!(syscall_name_to_nr("  ptrace  "), Some(SYS_PTRACE));
        assert_eq!(syscall_name_to_nr("\tbpf\t"), Some(SYS_BPF));
    }

    #[test]
    fn test_syscall_numbers_are_correct_x86_64() {
        assert_eq!(SYS_PTRACE, 101);
        assert_eq!(SYS_MOUNT, 165);
        assert_eq!(SYS_KEXEC_LOAD, 246);
        assert_eq!(SYS_UNSHARE, 272);
        assert_eq!(SYS_SETNS, 308);
        assert_eq!(SYS_BPF, 321);
        assert_eq!(SYS_SOCKET, 41);
        assert_eq!(SYS_CONNECT, 42);
        assert_eq!(SYS_ACCEPT, 43);
        assert_eq!(SYS_SENDTO, 44);
        assert_eq!(SYS_RECVFROM, 45);
        assert_eq!(SYS_SENDMSG, 46);
        assert_eq!(SYS_RECVMSG, 47);
        assert_eq!(SYS_BIND, 49);
        assert_eq!(SYS_LISTEN, 50);
    }

    // ── BPF constants ──────────────────────────────────────────────────────

    #[test]
    fn test_bpf_constants() {
        assert_eq!(OFFSET_NR, 0);
        assert_eq!(OFFSET_ARCH, 4);
        assert_eq!(AUDIT_ARCH_X86_64, 0xC000003E);
        assert_eq!(SECCOMP_RET_ALLOW, 0x7FFF0000);
        assert_eq!(SECCOMP_RET_ERRNO, 0x00050000);
        assert_eq!(EPERM, 1);
    }

    // ── bpf_stmt / bpf_jump ────────────────────────────────────────────────

    #[test]
    fn test_bpf_stmt_fields() {
        let s = bpf_stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_ARCH);
        assert_eq!(s.jt, 0);
        assert_eq!(s.jf, 0);
        assert_eq!(s.k, OFFSET_ARCH);
        assert_eq!(s.code, BPF_LD | BPF_W | BPF_ABS);
    }

    #[test]
    fn test_bpf_jump_fields() {
        let j = bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, SYS_PTRACE, 3, 0);
        assert_eq!(j.jt, 3);
        assert_eq!(j.jf, 0);
        assert_eq!(j.k, SYS_PTRACE);
    }

    #[test]
    fn test_bpf_stmt_ret_allow() {
        let s = bpf_stmt(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
        assert_eq!(s.k, SECCOMP_RET_ALLOW);
        assert_eq!(s.jt, 0);
        assert_eq!(s.jf, 0);
    }

    #[test]
    fn test_bpf_stmt_ret_deny() {
        let s = bpf_stmt(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
        assert_eq!(s.k, SECCOMP_RET_ERRNO | EPERM);
    }

    // ── filter construction logic ─────────────────────────────────────────

    fn build_filter(block_net: bool, allowed_syscalls: &[&str]) -> Vec<SockFilter> {
        let all_dangerous = vec![
            SYS_PTRACE, SYS_MOUNT, SYS_UNSHARE, SYS_KEXEC_LOAD, SYS_BPF, SYS_SETNS,
        ];
        let wildcard = allowed_syscalls.iter().any(|s| *s == "*");
        let mut blocked: Vec<u32> = if wildcard {
            Vec::new()
        } else {
            all_dangerous
                .into_iter()
                .filter(|&nr| {
                    !allowed_syscalls
                        .iter()
                        .any(|name| syscall_name_to_nr(name) == Some(nr))
                })
                .collect()
        };
        if block_net {
            blocked.push(SYS_SOCKET);
            blocked.push(SYS_CONNECT);
            blocked.push(SYS_ACCEPT);
            blocked.push(SYS_BIND);
            blocked.push(SYS_LISTEN);
        }
        let num_blocked = blocked.len();
        let mut filter: Vec<SockFilter> = Vec::new();
        filter.push(bpf_stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_ARCH));
        filter.push(bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH_X86_64, 0, (num_blocked + 2) as u8));
        filter.push(bpf_stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_NR));
        for (i, &nr) in blocked.iter().enumerate() {
            let jump_to_deny = (num_blocked - i) as u8;
            filter.push(bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, nr, jump_to_deny, 0));
        }
        filter.push(bpf_stmt(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));
        filter.push(bpf_stmt(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM));
        filter
    }

    #[test]
    fn test_filter_default_blocks_6_dangerous() {
        let filter = build_filter(false, &[]);
        assert_eq!(filter.len(), 11); // 3 preamble + 6 blocks + 2 ret
    }

    #[test]
    fn test_filter_with_block_network_adds_5() {
        let filter = build_filter(true, &[]);
        assert_eq!(filter.len(), 16); // 3 + 11 + 2
    }

    #[test]
    fn test_filter_wildcard_blocks_nothing() {
        let filter = build_filter(false, &["*"]);
        assert_eq!(filter.len(), 5); // 3 preamble + 2 ret
    }

    #[test]
    fn test_filter_wildcard_with_block_net() {
        let filter = build_filter(true, &["*"]);
        assert_eq!(filter.len(), 10); // 3 + 5 net + 2
    }

    #[test]
    fn test_filter_allow_ptrace_removes_from_blocked() {
        let filter = build_filter(false, &["ptrace"]);
        assert_eq!(filter.len(), 10); // 3 + 5 + 2
    }

    #[test]
    fn test_filter_allow_all_dangerous() {
        let filter = build_filter(false, &["ptrace", "mount", "unshare", "kexec_load", "bpf", "setns"]);
        assert_eq!(filter.len(), 5);
    }

    #[test]
    fn test_filter_preamble_loads_arch_first() {
        let filter = build_filter(false, &[]);
        assert_eq!(filter[0].k, OFFSET_ARCH);
    }

    #[test]
    fn test_filter_preamble_checks_x86_64_arch() {
        let filter = build_filter(false, &[]);
        assert_eq!(filter[1].k, AUDIT_ARCH_X86_64);
    }

    #[test]
    fn test_filter_preamble_loads_syscall_nr() {
        let filter = build_filter(false, &[]);
        assert_eq!(filter[2].k, OFFSET_NR);
    }

    #[test]
    fn test_filter_last_two_are_ret_allow_deny() {
        let filter = build_filter(false, &[]);
        let n = filter.len();
        assert_eq!(filter[n - 2].k, SECCOMP_RET_ALLOW);
        assert_eq!(filter[n - 1].k, SECCOMP_RET_ERRNO | EPERM);
    }

    #[test]
    fn test_filter_contains_ptrace_block() {
        let filter = build_filter(false, &[]);
        assert!(filter.iter().any(|f| f.k == SYS_PTRACE));
    }

    #[test]
    fn test_filter_block_net_contains_socket() {
        let filter = build_filter(true, &[]);
        assert!(filter.iter().any(|f| f.k == SYS_SOCKET));
    }

    #[test]
    fn test_filter_no_block_net_no_socket() {
        let filter = build_filter(false, &[]);
        assert!(!filter.iter().any(|f| f.k == SYS_SOCKET));
    }

    #[test]
    fn test_sockfilter_size() {
        // SockFilter: u16 + u8 + u8 + u32 = 8 bytes (repr C)
        assert_eq!(std::mem::size_of::<SockFilter>(), 8);
    }
}
