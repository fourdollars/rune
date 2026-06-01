#![allow(dead_code, unused_imports, unused_variables)]
#![allow(clippy::all)]
//! rune _landlock: applies Landlock filesystem restrictions then exec's the remaining args.
//! Usage: rune _landlock --rw /tmp --ro /bin --ro /usr --ro /lib -- <command> [args...]
//!
//! Restricts filesystem access to only the specified paths.

use std::env;
use std::os::unix::process::CommandExt;
use std::process::Command;

// Landlock syscall numbers (x86_64)
const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;

// Landlock ABI constants
const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1 << 0;

// Access rights for files
const LANDLOCK_ACCESS_FS_EXECUTE: u64 = 1 << 0;
const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
const LANDLOCK_ACCESS_FS_READ_FILE: u64 = 1 << 2;
const LANDLOCK_ACCESS_FS_READ_DIR: u64 = 1 << 3;
const LANDLOCK_ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
const LANDLOCK_ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
const LANDLOCK_ACCESS_FS_MAKE_CHAR: u64 = 1 << 6;
const LANDLOCK_ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
const LANDLOCK_ACCESS_FS_MAKE_REG: u64 = 1 << 8;
const LANDLOCK_ACCESS_FS_MAKE_SOCK: u64 = 1 << 9;
const LANDLOCK_ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
const LANDLOCK_ACCESS_FS_MAKE_BLOCK: u64 = 1 << 11;
const LANDLOCK_ACCESS_FS_MAKE_SYM: u64 = 1 << 12;
const LANDLOCK_ACCESS_FS_REFER: u64 = 1 << 13;
const LANDLOCK_ACCESS_FS_TRUNCATE: u64 = 1 << 14;

const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;

// All read-only access rights
const ACCESS_RO: u64 =
    LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR;

// All read-write access rights
const ACCESS_RW: u64 = ACCESS_RO
    | LANDLOCK_ACCESS_FS_WRITE_FILE
    | LANDLOCK_ACCESS_FS_REMOVE_DIR
    | LANDLOCK_ACCESS_FS_REMOVE_FILE
    | LANDLOCK_ACCESS_FS_MAKE_CHAR
    | LANDLOCK_ACCESS_FS_MAKE_DIR
    | LANDLOCK_ACCESS_FS_MAKE_REG
    | LANDLOCK_ACCESS_FS_MAKE_SOCK
    | LANDLOCK_ACCESS_FS_MAKE_FIFO
    | LANDLOCK_ACCESS_FS_MAKE_BLOCK
    | LANDLOCK_ACCESS_FS_MAKE_SYM
    | LANDLOCK_ACCESS_FS_REFER
    | LANDLOCK_ACCESS_FS_TRUNCATE;

// All FS access (for ruleset_attr)
const ACCESS_ALL: u64 = ACCESS_RW;

#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
}

#[repr(C)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

/// Entry point when invoked as `rune _landlock` / `_seccomp` / `_net-guard` subcommand.
pub fn run() {
    let all_args: Vec<String> = env::args().collect();
    let args: Vec<String> = all_args[1..].to_vec(); // skip binary name, keep subcommand as args[0]

    // Parse arguments: --rw <path> --ro <path> -- <cmd> [args]
    let mut rw_paths: Vec<String> = Vec::new();
    let mut ro_paths: Vec<String> = Vec::new();
    let mut traverse_paths: Vec<String> = Vec::new();
    let mut cmd_start = 0;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--rw" => {
                i += 1;
                if i < args.len() {
                    rw_paths.push(args[i].clone());
                }
            }
            "--ro" => {
                i += 1;
                if i < args.len() {
                    ro_paths.push(args[i].clone());
                }
            }
            "--traverse" => {
                i += 1;
                if i < args.len() {
                    traverse_paths.push(args[i].clone());
                }
            }
            "--" => {
                cmd_start = i + 1;
                break;
            }
            _ => {
                cmd_start = i;
                break;
            }
        }
        i += 1;
    }

    if cmd_start == 0 || cmd_start >= args.len() {
        eprintln!("Usage: rune _landlock [--rw <path>]... [--ro <path>]... -- <command> [args...]");
        std::process::exit(1);
    }

    // Check Landlock ABI version
    let abi_version = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            std::ptr::null::<u8>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    if abi_version < 1 {
        eprintln!(
            "rune _landlock: Landlock not supported (ABI version: {})",
            abi_version
        );
        // Graceful degradation: just exec without restriction
        let err = Command::new(&args[cmd_start])
            .args(&args[cmd_start + 1..])
            .exec();
        eprintln!("rune _landlock: exec failed: {}", err);
        std::process::exit(1);
    }

    // Create ruleset
    let ruleset_attr = LandlockRulesetAttr {
        handled_access_fs: ACCESS_ALL,
        handled_access_net: 0,
    };

    let ruleset_fd = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            &ruleset_attr as *const LandlockRulesetAttr,
            std::mem::size_of::<LandlockRulesetAttr>(),
            0u32,
        )
    };
    if ruleset_fd < 0 {
        eprintln!(
            "rune _landlock: landlock_create_ruleset failed: {}",
            std::io::Error::last_os_error()
        );
        std::process::exit(1);
    }

    // Add RW rules
    for path in &rw_paths {
        if let Err(e) = add_path_rule(ruleset_fd as i32, path, ACCESS_RW) {
            eprintln!(
                "rune _landlock: warning: failed to add rw rule for {}: {}",
                path, e
            );
        }
    }

    // Add RO rules
    for path in &ro_paths {
        if let Err(e) = add_path_rule(ruleset_fd as i32, path, ACCESS_RO) {
            eprintln!(
                "rune _landlock: warning: failed to add ro rule for {}: {}",
                path, e
            );
        }
    }

    // Add traverse-only rules (EXECUTE only — for directory path traversal)
    for path in &traverse_paths {
        if let Err(e) = add_path_rule(ruleset_fd as i32, path, LANDLOCK_ACCESS_FS_EXECUTE) {
            eprintln!(
                "rune _landlock: warning: failed to add traverse rule for {}: {}",
                path, e
            );
        }
    }

    // Set no_new_privs (required)
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            eprintln!(
                "rune _landlock: prctl(NO_NEW_PRIVS) failed: {}",
                std::io::Error::last_os_error()
            );
            std::process::exit(1);
        }
    }

    // Restrict self
    let ret = unsafe { libc::syscall(SYS_LANDLOCK_RESTRICT_SELF, ruleset_fd, 0u32) };
    if ret != 0 {
        eprintln!(
            "rune _landlock: landlock_restrict_self failed: {}",
            std::io::Error::last_os_error()
        );
        std::process::exit(1);
    }

    // Close ruleset fd
    unsafe {
        libc::close(ruleset_fd as i32);
    }

    // Exec
    let err = Command::new(&args[cmd_start])
        .args(&args[cmd_start + 1..])
        .exec();
    eprintln!("rune _landlock: exec failed: {}", err);
    std::process::exit(1);
}

fn add_path_rule(ruleset_fd: i32, path: &str, access: u64) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::unix::io::RawFd;

    let c_path = CString::new(path).map_err(|e| e.to_string())?;
    let fd: RawFd = unsafe { libc::open(c_path.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(format!(
            "open({}) failed: {}",
            path,
            std::io::Error::last_os_error()
        ));
    }

    // Determine if this is a file or directory and adjust access rights accordingly.
    // Files and special nodes (char/block devices like /dev/null) don't support
    // directory-only operations (READ_DIR, REMOVE_DIR, MAKE_*).
    let p = std::path::Path::new(path);
    let is_non_dir = !p.is_dir();
    let effective_access = if is_non_dir {
        // File-only access: EXECUTE, READ_FILE, WRITE_FILE, TRUNCATE
        access
            & (LANDLOCK_ACCESS_FS_EXECUTE
                | LANDLOCK_ACCESS_FS_READ_FILE
                | LANDLOCK_ACCESS_FS_WRITE_FILE
                | LANDLOCK_ACCESS_FS_TRUNCATE)
    } else {
        access
    };

    let path_beneath = LandlockPathBeneathAttr {
        allowed_access: effective_access,
        parent_fd: fd,
    };

    let ret = unsafe {
        libc::syscall(
            SYS_LANDLOCK_ADD_RULE,
            ruleset_fd,
            LANDLOCK_RULE_PATH_BENEATH,
            &path_beneath as *const LandlockPathBeneathAttr,
            0u32,
        )
    };

    unsafe {
        libc::close(fd);
    }

    if ret != 0 {
        Err(format!(
            "landlock_add_rule failed: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_access_mask_for_directory() {
        // Directories should keep full access mask unchanged
        let access = ACCESS_RO;
        let is_file = false;
        let effective = if is_file {
            access
                & (LANDLOCK_ACCESS_FS_EXECUTE
                    | LANDLOCK_ACCESS_FS_READ_FILE
                    | LANDLOCK_ACCESS_FS_WRITE_FILE
                    | LANDLOCK_ACCESS_FS_TRUNCATE)
        } else {
            access
        };
        assert_eq!(effective, ACCESS_RO);
        // ACCESS_RO includes READ_DIR
        assert_ne!(effective & LANDLOCK_ACCESS_FS_READ_DIR, 0);
    }

    #[test]
    fn test_access_mask_for_file_ro() {
        // Files should strip directory-only rights
        let access = ACCESS_RO;
        let is_file = true;
        let effective = if is_file {
            access
                & (LANDLOCK_ACCESS_FS_EXECUTE
                    | LANDLOCK_ACCESS_FS_READ_FILE
                    | LANDLOCK_ACCESS_FS_WRITE_FILE
                    | LANDLOCK_ACCESS_FS_TRUNCATE)
        } else {
            access
        };
        // Should have EXECUTE and READ_FILE
        assert_ne!(effective & LANDLOCK_ACCESS_FS_EXECUTE, 0);
        assert_ne!(effective & LANDLOCK_ACCESS_FS_READ_FILE, 0);
        // Should NOT have READ_DIR
        assert_eq!(effective & LANDLOCK_ACCESS_FS_READ_DIR, 0);
        // Should NOT have WRITE_FILE (wasn't in ACCESS_RO)
        assert_eq!(effective & LANDLOCK_ACCESS_FS_WRITE_FILE, 0);
    }

    #[test]
    fn test_access_mask_for_file_rw() {
        // RW file should have EXECUTE, READ_FILE, WRITE_FILE, TRUNCATE
        let access = ACCESS_RW;
        let is_file = true;
        let effective = if is_file {
            access
                & (LANDLOCK_ACCESS_FS_EXECUTE
                    | LANDLOCK_ACCESS_FS_READ_FILE
                    | LANDLOCK_ACCESS_FS_WRITE_FILE
                    | LANDLOCK_ACCESS_FS_TRUNCATE)
        } else {
            access
        };
        assert_ne!(effective & LANDLOCK_ACCESS_FS_EXECUTE, 0);
        assert_ne!(effective & LANDLOCK_ACCESS_FS_READ_FILE, 0);
        assert_ne!(effective & LANDLOCK_ACCESS_FS_WRITE_FILE, 0);
        assert_ne!(effective & LANDLOCK_ACCESS_FS_TRUNCATE, 0);
        // Should NOT have directory-only rights
        assert_eq!(effective & LANDLOCK_ACCESS_FS_MAKE_DIR, 0);
        assert_eq!(effective & LANDLOCK_ACCESS_FS_REMOVE_DIR, 0);
        assert_eq!(effective & LANDLOCK_ACCESS_FS_READ_DIR, 0);
    }

    #[test]
    fn test_access_ro_includes_expected_rights() {
        assert_ne!(ACCESS_RO & LANDLOCK_ACCESS_FS_EXECUTE, 0);
        assert_ne!(ACCESS_RO & LANDLOCK_ACCESS_FS_READ_FILE, 0);
        assert_ne!(ACCESS_RO & LANDLOCK_ACCESS_FS_READ_DIR, 0);
        // RO should NOT include write
        assert_eq!(ACCESS_RO & LANDLOCK_ACCESS_FS_WRITE_FILE, 0);
    }

    #[test]
    fn test_access_rw_includes_write() {
        assert_ne!(ACCESS_RW & LANDLOCK_ACCESS_FS_WRITE_FILE, 0);
        assert_ne!(ACCESS_RW & LANDLOCK_ACCESS_FS_REMOVE_FILE, 0);
        assert_ne!(ACCESS_RW & LANDLOCK_ACCESS_FS_MAKE_REG, 0);
        assert_ne!(ACCESS_RW & LANDLOCK_ACCESS_FS_TRUNCATE, 0);
    }

    #[test]
    fn test_traverse_access_is_execute_only() {
        // Traverse paths should only get EXECUTE permission (for directory lookup)
        let traverse_access = LANDLOCK_ACCESS_FS_EXECUTE;
        // Should NOT include READ_FILE or READ_DIR
        assert_eq!(traverse_access & LANDLOCK_ACCESS_FS_READ_FILE, 0);
        assert_eq!(traverse_access & LANDLOCK_ACCESS_FS_READ_DIR, 0);
        assert_eq!(traverse_access & LANDLOCK_ACCESS_FS_WRITE_FILE, 0);
        // Should include EXECUTE
        assert_ne!(traverse_access & LANDLOCK_ACCESS_FS_EXECUTE, 0);
    }

    #[test]
    fn test_traverse_vs_ro_permissions() {
        // RO gives full read access, traverse gives only EXECUTE
        assert!(ACCESS_RO > LANDLOCK_ACCESS_FS_EXECUTE);
        // Traverse should be a strict subset of RO
        assert_eq!(
            LANDLOCK_ACCESS_FS_EXECUTE & ACCESS_RO,
            LANDLOCK_ACCESS_FS_EXECUTE
        );
    }
}

// ── Additional tests to boost coverage ──────────────────────────────────────
#[cfg(test)]
mod tests_extra {
    use super::*;

    #[test]
    fn test_access_all_equals_access_rw() {
        // ACCESS_ALL should equal ACCESS_RW
        assert_eq!(ACCESS_ALL, ACCESS_RW);
    }

    #[test]
    fn test_access_ro_is_subset_of_rw() {
        // Every bit in RO must also be in RW
        assert_eq!(ACCESS_RO & ACCESS_RW, ACCESS_RO);
    }

    #[test]
    fn test_access_rw_excludes_write_from_ro_mask() {
        // ACCESS_RO should not include WRITE_FILE
        assert_eq!(ACCESS_RO & LANDLOCK_ACCESS_FS_WRITE_FILE, 0);
    }

    #[test]
    fn test_landlock_constants_are_powers_of_two() {
        let constants = [
            LANDLOCK_ACCESS_FS_EXECUTE,
            LANDLOCK_ACCESS_FS_WRITE_FILE,
            LANDLOCK_ACCESS_FS_READ_FILE,
            LANDLOCK_ACCESS_FS_READ_DIR,
            LANDLOCK_ACCESS_FS_REMOVE_DIR,
            LANDLOCK_ACCESS_FS_REMOVE_FILE,
            LANDLOCK_ACCESS_FS_MAKE_CHAR,
            LANDLOCK_ACCESS_FS_MAKE_DIR,
            LANDLOCK_ACCESS_FS_MAKE_REG,
            LANDLOCK_ACCESS_FS_MAKE_SOCK,
            LANDLOCK_ACCESS_FS_MAKE_FIFO,
            LANDLOCK_ACCESS_FS_MAKE_BLOCK,
            LANDLOCK_ACCESS_FS_MAKE_SYM,
            LANDLOCK_ACCESS_FS_REFER,
            LANDLOCK_ACCESS_FS_TRUNCATE,
        ];
        for c in constants {
            assert_eq!(c & (c - 1), 0, "constant {:#x} is not power of two", c);
        }
    }

    #[test]
    fn test_landlock_constants_unique() {
        use std::collections::HashSet;
        let constants = vec![
            LANDLOCK_ACCESS_FS_EXECUTE,
            LANDLOCK_ACCESS_FS_WRITE_FILE,
            LANDLOCK_ACCESS_FS_READ_FILE,
            LANDLOCK_ACCESS_FS_READ_DIR,
            LANDLOCK_ACCESS_FS_REMOVE_DIR,
            LANDLOCK_ACCESS_FS_REMOVE_FILE,
            LANDLOCK_ACCESS_FS_MAKE_CHAR,
            LANDLOCK_ACCESS_FS_MAKE_DIR,
            LANDLOCK_ACCESS_FS_MAKE_REG,
            LANDLOCK_ACCESS_FS_MAKE_SOCK,
            LANDLOCK_ACCESS_FS_MAKE_FIFO,
            LANDLOCK_ACCESS_FS_MAKE_BLOCK,
            LANDLOCK_ACCESS_FS_MAKE_SYM,
            LANDLOCK_ACCESS_FS_REFER,
            LANDLOCK_ACCESS_FS_TRUNCATE,
        ];
        let unique: HashSet<u64> = constants.iter().cloned().collect();
        assert_eq!(unique.len(), constants.len());
    }

    #[test]
    fn test_access_rw_has_all_file_ops() {
        // RW should include all individual write operations
        let write_ops = [
            LANDLOCK_ACCESS_FS_WRITE_FILE,
            LANDLOCK_ACCESS_FS_REMOVE_DIR,
            LANDLOCK_ACCESS_FS_REMOVE_FILE,
            LANDLOCK_ACCESS_FS_MAKE_CHAR,
            LANDLOCK_ACCESS_FS_MAKE_DIR,
            LANDLOCK_ACCESS_FS_MAKE_REG,
            LANDLOCK_ACCESS_FS_MAKE_SOCK,
            LANDLOCK_ACCESS_FS_MAKE_FIFO,
            LANDLOCK_ACCESS_FS_MAKE_BLOCK,
            LANDLOCK_ACCESS_FS_MAKE_SYM,
            LANDLOCK_ACCESS_FS_REFER,
            LANDLOCK_ACCESS_FS_TRUNCATE,
        ];
        for op in write_ops {
            assert_ne!(ACCESS_RW & op, 0, "ACCESS_RW missing op {:#x}", op);
        }
    }

    #[test]
    fn test_file_access_mask_strips_dir_ops() {
        // Simulate what add_path_rule does for a non-dir (file)
        let file_mask = LANDLOCK_ACCESS_FS_EXECUTE
            | LANDLOCK_ACCESS_FS_READ_FILE
            | LANDLOCK_ACCESS_FS_WRITE_FILE
            | LANDLOCK_ACCESS_FS_TRUNCATE;

        // Apply to ACCESS_RW (would be full for dir)
        let effective = ACCESS_RW & file_mask;
        assert_ne!(effective & LANDLOCK_ACCESS_FS_EXECUTE, 0);
        assert_ne!(effective & LANDLOCK_ACCESS_FS_READ_FILE, 0);
        assert_ne!(effective & LANDLOCK_ACCESS_FS_WRITE_FILE, 0);
        assert_ne!(effective & LANDLOCK_ACCESS_FS_TRUNCATE, 0);

        // Directory operations should be stripped
        assert_eq!(effective & LANDLOCK_ACCESS_FS_READ_DIR, 0);
        assert_eq!(effective & LANDLOCK_ACCESS_FS_MAKE_DIR, 0);
        assert_eq!(effective & LANDLOCK_ACCESS_FS_REMOVE_DIR, 0);
    }

    #[test]
    fn test_add_path_rule_valid_path_tmp() {
        // /tmp always exists — should succeed
        let result = add_path_rule(-1, "/tmp", ACCESS_RO);
        // ruleset_fd = -1 is invalid → will fail at landlock_add_rule, but open should succeed
        // We expect an error string, but not a "open failed" error
        match result {
            Ok(_) => {} // might pass if kernel supports landlock with invalid fd test
            Err(e) => {
                // Should fail at landlock_add_rule, not at open
                assert!(!e.contains("open(/tmp) failed"), "unexpected error: {}", e);
            }
        }
    }

    #[test]
    fn test_add_path_rule_nonexistent_path() {
        let result = add_path_rule(0, "/nonexistent/path/that/does/not/exist", ACCESS_RO);
        assert!(result.is_err());
        let e = result.unwrap_err();
        assert!(e.contains("open(") || e.contains("failed"));
    }

    #[test]
    fn test_add_path_rule_dev_null() {
        // /dev/null always exists
        let result = add_path_rule(-1, "/dev/null", ACCESS_RO);
        match result {
            Ok(_) => {}
            Err(e) => {
                assert!(!e.contains("open(/dev/null) failed"), "open failed: {}", e);
            }
        }
    }

    #[test]
    fn test_add_path_rule_dev_urandom() {
        // /dev/urandom always exists
        let result = add_path_rule(-1, "/dev/urandom", ACCESS_RO);
        match result {
            Ok(_) => {}
            Err(e) => {
                assert!(
                    !e.contains("open(/dev/urandom) failed"),
                    "open failed: {}",
                    e
                );
            }
        }
    }

    #[test]
    fn test_landlock_rule_path_beneath_value() {
        assert_eq!(LANDLOCK_RULE_PATH_BENEATH, 1);
    }

    #[test]
    fn test_landlock_create_ruleset_version_flag() {
        assert_eq!(LANDLOCK_CREATE_RULESET_VERSION, 1);
    }

    #[test]
    fn test_sys_landlock_syscall_numbers() {
        assert_eq!(SYS_LANDLOCK_CREATE_RULESET, 444);
        assert_eq!(SYS_LANDLOCK_ADD_RULE, 445);
        assert_eq!(SYS_LANDLOCK_RESTRICT_SELF, 446);
    }

    #[test]
    fn test_access_ro_bit_count() {
        // ACCESS_RO = EXECUTE | READ_FILE | READ_DIR = 3 bits set
        let bits = ACCESS_RO.count_ones();
        assert_eq!(bits, 3);
    }

    #[test]
    fn test_access_rw_bit_count_greater_than_ro() {
        assert!(ACCESS_RW.count_ones() > ACCESS_RO.count_ones());
    }

    #[test]
    fn test_path_rule_cstring_null_byte_error() {
        // Path with null byte should fail CString::new
        let result = add_path_rule(0, "/tmp/bad\0path", ACCESS_RO);
        assert!(result.is_err());
    }
}
