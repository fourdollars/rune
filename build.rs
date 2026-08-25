use std::fs;
use std::path::Path;
use std::path::PathBuf;

fn compress_tree(src_dir: &Path, dst_dir: &Path) {
    fs::create_dir_all(dst_dir).unwrap();
    for entry in fs::read_dir(src_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        if path.is_dir() {
            compress_tree(&path, &dst_dir.join(&name));
            continue;
        }
        if !path.is_file() {
            continue;
        }
        // Cargo does not recurse into subdirectories for rerun-if-changed on a
        // directory path, so every nested asset is registered individually.
        println!("cargo:rerun-if-changed={}", path.display());
        let data = fs::read(&path).unwrap();
        let compressed = zstd::encode_all(data.as_slice(), 19).unwrap();
        fs::write(dst_dir.join(format!("{name}.zst")), &compressed).unwrap();
    }
}

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    compress_tree(Path::new("web"), &out_dir.join("web-zst"));
    println!("cargo:rerun-if-changed=web/");

    // Embed git commit hash
    let git_hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_HASH={}", git_hash);

    // Embed build date (UTC)
    let build_date = std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%d"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=BUILD_DATE={}", build_date);

    // Rerun if HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/");
}
