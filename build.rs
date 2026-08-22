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
}
