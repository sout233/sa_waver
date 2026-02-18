pub fn get_presets() -> Option<Vec<String>> {
    let config_dir = directories::BaseDirs::new()?
        .config_dir()
        .join("Sout Audio")
        .join("SA Waver");

    if !config_dir.exists() {
        std::fs::create_dir_all(&config_dir).ok();
        return Some(vec![]);
    }

    let entries = std::fs::read_dir(&config_dir).ok()?;

    let presets = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.is_file() && path.extension() == Some("ron".as_ref()) {
                // path.file_name()?.to_str().map(|s| s.to_string())
                Some(path.to_string_lossy().to_string())
            } else {
                None
            }
        })
        .collect();

    Some(presets)
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
