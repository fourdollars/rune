//! Embedded Plugin Installer for Vim and Neovim (`rune vim install`).

use std::fs;
use std::path::PathBuf;

pub const RUNE_VIM_SCRIPT: &str = include_str!("rune.vim");

pub fn install_plugin() -> anyhow::Result<()> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| anyhow::anyhow!("HOME environment variable not set"))?;

    let vim_target = home.join(".vim").join("plugin").join("rune.vim");
    let nvim_target = home
        .join(".config")
        .join("nvim")
        .join("plugin")
        .join("rune.vim");

    let mut installed_paths = Vec::new();

    if let Some(parent) = vim_target.parent() {
        fs::create_dir_all(parent)?;
        fs::write(&vim_target, RUNE_VIM_SCRIPT)?;
        installed_paths.push(vim_target);
    }

    if let Some(parent) = nvim_target.parent() {
        fs::create_dir_all(parent)?;
        fs::write(&nvim_target, RUNE_VIM_SCRIPT)?;
        installed_paths.push(nvim_target);
    }

    println!("✅ Rune Vim plugin installed successfully:");
    for path in installed_paths {
        println!("   - {}", path.display());
    }

    Ok(())
}
