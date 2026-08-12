use std::fs;
use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let web_zst_dir = out_dir.join("web-zst");
    fs::create_dir_all(&web_zst_dir).unwrap();

    let web_dir = PathBuf::from("web");
    for entry in fs::read_dir(&web_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let filename = path.file_name().unwrap().to_str().unwrap();
        let data = fs::read(&path).unwrap();
        let compressed = zstd::encode_all(data.as_slice(), 19).unwrap();
        fs::write(web_zst_dir.join(format!("{filename}.zst")), &compressed).unwrap();
    }

    println!("cargo:rerun-if-changed=web/");
}
