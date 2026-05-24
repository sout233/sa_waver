fn collect_ron_files_recursive(dir: &std::path::Path, presets: &mut Vec<String>) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_ron_files_recursive(&path, presets)?;
        } else if path.is_file() && path.extension() == Some("ron".as_ref()) {
            presets.push(path.to_string_lossy().to_string());
        }
    }

    Ok(())
}

pub fn get_presets() -> Option<Vec<String>> {
    let config_dir = directories::BaseDirs::new()?
        .config_dir()
        .join("Sout Audio")
        .join("SA Waver");

    if !config_dir.exists() {
        std::fs::create_dir_all(&config_dir).ok();
        return Some(vec![]);
    }

    let mut presets = Vec::new();
    collect_ron_files_recursive(&config_dir, &mut presets).ok()?;
    presets.sort();

    Some(presets)
}

pub fn get_builtin_presets() -> Vec<String> {
    let presets_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("presets");
    let mut presets = Vec::new();
    let _ = collect_ron_files_recursive(&presets_dir, &mut presets);
    presets.sort();
    presets
}


pub fn build_preset_path(file_name: &str) -> std::io::Result<String> {
    let safe_name: String = file_name
        .chars()
        .map(|c| match c {
            '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();

    let base_dirs = directories::BaseDirs::new().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Failed to get base directories")
    })?;

    let config_dir = base_dirs
        .config_dir()
        .join("Sout Audio")
        .join("SA Waver");

    std::fs::create_dir_all(&config_dir)?;

    let file_path = config_dir.join(format!("{}.ron", safe_name));

    if !file_path.exists() {
        std::fs::File::create(&file_path)?;
    }

    Ok(file_path.to_string_lossy().to_string())
}
