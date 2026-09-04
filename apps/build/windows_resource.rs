#[cfg(windows)]
pub fn configure(
    res: &mut winres::WindowsResource,
    file_description: &str,
    product_name: &str,
    original_filename: &str,
) {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    let version_four_part = to_four_part_version(&version);
    let version_words = to_version_words(&version_four_part);

    res.set("CompanyName", "Talos");
    res.set("FileDescription", file_description);
    res.set("FileVersion", &version_four_part);
    res.set("InternalName", original_filename);
    res.set("LegalCopyright", "Copyright (C) Talos");
    res.set("OriginalFilename", original_filename);
    res.set("ProductName", product_name);
    res.set("ProductVersion", &version_four_part);
    res.set_language(0x0809);
    res.set_version_info(winres::VersionInfo::FILEVERSION, version_words);
    res.set_version_info(winres::VersionInfo::PRODUCTVERSION, version_words);
}

#[cfg(windows)]
fn to_four_part_version(version: &str) -> String {
    let mut parts = version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse::<u16>().unwrap_or(0))
        .collect::<Vec<_>>();
    while parts.len() < 4 {
        parts.push(0);
    }
    format!("{}.{}.{}.{}", parts[0], parts[1], parts[2], parts[3])
}

#[cfg(windows)]
fn to_version_words(version: &str) -> u64 {
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u16>().unwrap_or(0) as u64)
        .collect::<Vec<_>>();
    while parts.len() < 4 {
        parts.push(0);
    }
    (parts[0] << 48) | (parts[1] << 32) | (parts[2] << 16) | parts[3]
}
