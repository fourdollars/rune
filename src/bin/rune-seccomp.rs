#![allow(dead_code, unused_imports, unused_variables)]
#![allow(clippy::all)]
//! rune-seccomp: applies a seccomp BPF filter then exec's the remaining args.
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

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Usage: rune-seccomp [--block-network] [--allow-syscalls <list>] <command> [args...]"
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
            eprintln!("rune-seccomp: prctl(NO_NEW_PRIVS) failed");
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
            eprintln!("rune-seccomp: prctl(SET_SECCOMP) failed");
            std::process::exit(1);
        }
    }

    let err = Command::new(&args[cmd_idx])
        .args(&args[cmd_idx + 1..])
        .exec();
    eprintln!("rune-seccomp: exec failed: {}", err);
    std::process::exit(1);
}
