use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;
use xxhash_rust::xxh3::Xxh3;

pub fn get_all_file_path(dir: &Path) -> Vec<PathBuf> {
    let mut file_paths = Vec::new();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                file_paths.push(path);
            } else if path.is_dir() {
                file_paths.extend(get_all_file_path(&path));
            }
        }
    }
    file_paths
}

pub async fn compute_hash(file_path: &PathBuf) -> tokio::io::Result<u128> {
    let file = tokio::fs::File::open(file_path).await?;
    let mut reader = tokio::io::BufReader::new(file);
    let mut xxh3 = Xxh3::new();
    let mut buf = vec![0; 32768];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        xxh3.update(&buf[..n]);
    }
    let xxh3_hash = xxh3.digest128();
    Ok(xxh3_hash)
}

pub fn export_all_hash(
    hash_file_path: &Path,
    hashes: &HashMap<PathBuf, u128>,
    file_paths: &[PathBuf],
    folder_path: &Path,
) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(hash_file_path)?;

    for file_path in file_paths {
        let hash = match hashes.get(file_path) {
            Some(hash) => hash,
            None => {
                return Err(tokio::io::Error::new(
                    tokio::io::ErrorKind::Other,
                    format!("排序哈希时找不到[{}]的哈希", file_path.display()),
                ));
            }
        };
        writeln!(
            file,
            "[{} | {:x}]",
            file_path.strip_prefix(folder_path).unwrap().display(),
            hash
        )?;
    }
    Ok(())
}

pub fn read_hash_file(
    folder_path: &Path,
    hash_file_path: &Path,
) -> io::Result<HashMap<PathBuf, u128>> {
    let mut hash_map = HashMap::new();

    let file = File::open(hash_file_path)?;
    let reader = BufReader::new(file);

    for line in reader.lines().flatten() {
        let parts: Vec<&str> = line
            .trim_matches(|c| c == '[' || c == ']' || c == ' ')
            .split(" | ")
            .collect();
        if parts.len() == 2 {
            let mut key = folder_path.to_path_buf();
            key.push(parts[0]);
            let value = match u128::from_str_radix(parts[1], 16) {
                Ok(value) => value,
                Err(err) => {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!("无法把[{}]转换为u128: {}", parts[1], err),
                    ))
                }
            };
            hash_map.insert(key, value);
        }
    }
    Ok(hash_map)
}
