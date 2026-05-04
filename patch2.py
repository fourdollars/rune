import sys

with open("src/sandbox/mod.rs", "r") as f:
    content = f.read()

find_fn = """    /// Locate the librune_dns_filter.so library.
    fn find_dns_filter_lib() -> Option<String> {
        let candidates = [
            "/usr/local/lib/librune_dns_filter.so",
            "/usr/lib/librune_dns_filter.so",
        ];
        for path in &candidates {
            if std::path::Path::new(path).exists() {
                return Some(path.to_string());
            }
        }
        // Also check next to the current binary
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let lib = dir.join("librune_dns_filter.so");
                if lib.exists() {
                    return lib.to_str().map(|s| s.to_string());
                }
            }
        }
        None
    }"""

content = content.replace(find_fn, "")

with open("src/sandbox/mod.rs", "w") as f:
    f.write(content)
