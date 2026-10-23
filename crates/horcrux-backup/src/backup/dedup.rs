//! Horcrux-native deduplicating archive format (`compression: "dedup"`).
//!
//! This is NOT the Proxmox Backup Server protocol: it is a content-addressed
//! chunk store that sits next to the archives of one backup storage.
//!
//! - every payload file (manifest, disks, rootfs) is cut into fixed 4 MiB
//!   chunks; a chunk is named by the SHA-256 of its bytes and stored once, gzip
//!   compressed, at `<storage>/.hxchunks/ab/cd/<sha256>`;
//! - the "archive" is a small JSON index (`*.hxidx`) listing each file's
//!   chunks, so a second backup of a guest that mostly didn't change only
//!   writes the chunks that changed (incremental), and identical chunks of
//!   different guests are stored once;
//! - reading a chunk re-hashes it, so extract and verify detect any damage;
//! - deleting a backup runs a garbage collection that removes the chunks no
//!   index refers to any more.
use horcrux_common::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const CHUNK_SIZE: usize = 4 * 1024 * 1024;
pub const INDEX_EXT: &str = "hxidx";
const CHUNK_DIR: &str = ".hxchunks";
const INDEX_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct IndexFile {
    name: String,
    size: u64,
    chunks: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Index {
    version: u32,
    chunk_size: usize,
    files: Vec<IndexFile>,
}

/// What a backup wrote.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DedupStats {
    pub chunks: usize,
    pub new_chunks: usize,
    /// Bytes of chunks written by this backup (after compression).
    pub new_bytes: u64,
}

fn chunk_root(index: &Path) -> PathBuf {
    index
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(CHUNK_DIR)
}

fn chunk_path(root: &Path, hash: &str) -> PathBuf {
    root.join(&hash[0..2]).join(&hash[2..4]).join(hash)
}

fn valid_hash(h: &str) -> bool {
    h.len() == 64
        && h.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn io(e: std::io::Error) -> Error {
    Error::System(format!("backup chunk store: {e}"))
}

fn read_full(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        let r = f.read(&mut buf[n..])?;
        if r == 0 {
            break;
        }
        n += r;
    }
    Ok(n)
}

/// Cut `files` (relative to `dir`) into chunks and write the index `out`.
pub fn write_dedup(dir: &Path, files: &[String], out: &Path) -> Result<DedupStats> {
    let root = chunk_root(out);
    std::fs::create_dir_all(&root).map_err(io)?;
    let mut stats = DedupStats::default();
    let mut index = Index {
        version: INDEX_VERSION,
        chunk_size: CHUNK_SIZE,
        files: Vec::new(),
    };
    let mut buf = vec![0u8; CHUNK_SIZE];
    for name in files {
        let mut f = std::fs::File::open(dir.join(name)).map_err(io)?;
        let mut entry = IndexFile {
            name: name.clone(),
            size: 0,
            chunks: Vec::new(),
        };
        loop {
            let n = read_full(&mut f, &mut buf).map_err(io)?;
            if n == 0 {
                break;
            }
            let hash = hex::encode(Sha256::digest(&buf[..n]));
            let path = chunk_path(&root, &hash);
            stats.chunks += 1;
            if !path.exists() {
                std::fs::create_dir_all(path.parent().expect("chunk path has a parent"))
                    .map_err(io)?;
                let tmp = path.with_extension(format!("tmp{}", std::process::id()));
                let mut enc =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                enc.write_all(&buf[..n]).map_err(io)?;
                let packed = enc.finish().map_err(io)?;
                std::fs::write(&tmp, &packed).map_err(io)?;
                std::fs::rename(&tmp, &path).map_err(io)?;
                stats.new_chunks += 1;
                stats.new_bytes += packed.len() as u64;
            }
            entry.size += n as u64;
            entry.chunks.push(hash);
        }
        index.files.push(entry);
    }
    let partial = PathBuf::from(format!("{}.partial", out.display()));
    let json = serde_json::to_vec(&index)
        .map_err(|e| Error::System(format!("serialize backup index: {e}")))?;
    let res = std::fs::write(&partial, &json).and_then(|_| std::fs::rename(&partial, out));
    if let Err(e) = res {
        let _ = std::fs::remove_file(&partial);
        return Err(io(e));
    }
    Ok(stats)
}

fn read_index(path: &Path) -> Result<Index> {
    let raw = std::fs::read(path)
        .map_err(|e| Error::System(format!("backup index {}: {e}", path.display())))?;
    let idx: Index = serde_json::from_slice(&raw)
        .map_err(|e| Error::System(format!("invalid backup index {}: {e}", path.display())))?;
    if idx.version > INDEX_VERSION {
        return Err(Error::System(format!(
            "backup index version {} is newer than this Horcrux supports",
            idx.version
        )));
    }
    Ok(idx)
}

/// One chunk's bytes, checked against its name.
fn load_chunk(root: &Path, hash: &str) -> Result<Vec<u8>> {
    if !valid_hash(hash) {
        return Err(Error::System(format!(
            "invalid chunk id {hash:?} in the index"
        )));
    }
    let path = chunk_path(root, hash);
    let packed = std::fs::read(&path)
        .map_err(|e| Error::System(format!("chunk {hash} is missing or unreadable: {e}")))?;
    let mut data = Vec::new();
    flate2::read::GzDecoder::new(&packed[..])
        .read_to_end(&mut data)
        .map_err(|e| Error::System(format!("chunk {hash} is corrupted: {e}")))?;
    let got = hex::encode(Sha256::digest(&data));
    if got != hash {
        return Err(Error::System(format!(
            "chunk {hash} is corrupted (its content hashes to {got})"
        )));
    }
    Ok(data)
}

/// Rebuild the files of the backup `index` into `dir`.
pub fn extract_dedup(index: &Path, dir: &Path) -> Result<()> {
    let idx = read_index(index)?;
    let root = chunk_root(index);
    for f in &idx.files {
        if f.name.contains('/') || f.name.starts_with('.') {
            return Err(Error::System(format!(
                "invalid file name {:?} in the index",
                f.name
            )));
        }
        let mut out = std::fs::File::create(dir.join(&f.name)).map_err(io)?;
        let mut total = 0u64;
        for h in &f.chunks {
            let data = load_chunk(&root, h)?;
            total += data.len() as u64;
            out.write_all(&data).map_err(io)?;
        }
        if total != f.size {
            return Err(Error::System(format!(
                "{} rebuilt to {total} bytes, the index says {}",
                f.name, f.size
            )));
        }
    }
    Ok(())
}

/// Check every chunk the index refers to (exists, decompresses, hashes to
/// its name); returns the number of chunks checked.
pub fn verify_chunks(index: &Path) -> Result<usize> {
    let idx = read_index(index)?;
    let root = chunk_root(index);
    let mut n = 0;
    for f in &idx.files {
        for h in &f.chunks {
            load_chunk(&root, h)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Remove chunks no index in `storage` refers to. A chunk younger than
/// `grace` is kept: a backup that is still running has written chunks but not
/// yet its index. Returns (chunks removed, bytes freed).
pub fn gc(storage: &Path, grace: std::time::Duration) -> Result<(usize, u64)> {
    let root = storage.join(CHUNK_DIR);
    if !root.is_dir() {
        return Ok((0, 0));
    }
    let mut live: HashSet<String> = HashSet::new();
    for e in std::fs::read_dir(storage).map_err(io)?.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == INDEX_EXT) {
            // an unreadable index could refer to anything: keep everything
            let idx = read_index(&p)?;
            live.extend(idx.files.into_iter().flat_map(|f| f.chunks));
        }
    }
    let (mut removed, mut freed) = (0, 0);
    let now = std::time::SystemTime::now();
    for a in std::fs::read_dir(&root).map_err(io)?.flatten() {
        for b in std::fs::read_dir(a.path()).into_iter().flatten().flatten() {
            for c in std::fs::read_dir(b.path()).into_iter().flatten().flatten() {
                let name = c.file_name().to_string_lossy().to_string();
                if !valid_hash(&name) || live.contains(&name) {
                    continue;
                }
                let Ok(md) = c.metadata() else { continue };
                let age = md
                    .modified()
                    .ok()
                    .and_then(|m| now.duration_since(m).ok())
                    .unwrap_or_default();
                if age >= grace && std::fs::remove_file(c.path()).is_ok() {
                    removed += 1;
                    freed += md.len();
                }
            }
            let _ = std::fs::remove_dir(b.path());
        }
        let _ = std::fs::remove_dir(a.path());
    }
    let _ = std::fs::remove_dir(&root);
    Ok((removed, freed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(len: usize, seed: u32) -> Vec<u8> {
        let mut x: u64 = 0x9e3779b97f4a7c15 ^ u64::from(seed);
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    fn chunk_files(storage: &Path) -> usize {
        let root = storage.join(CHUNK_DIR);
        walk(&root)
    }
    fn walk(p: &Path) -> usize {
        std::fs::read_dir(p)
            .map(|rd| {
                rd.flatten()
                    .map(|e| {
                        if e.path().is_dir() {
                            walk(&e.path())
                        } else {
                            1
                        }
                    })
                    .sum()
            })
            .unwrap_or(0)
    }

    #[test]
    fn second_backup_only_stores_changed_chunks_and_roundtrips() {
        let src = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let big = data(CHUNK_SIZE * 2 + 1000, 1);
        std::fs::write(src.path().join("disk-0.img"), &big).unwrap();
        std::fs::write(src.path().join("manifest.json"), b"{\"a\":1}").unwrap();
        let names = vec!["manifest.json".to_string(), "disk-0.img".to_string()];
        let idx1 = store.path().join("one.hxidx");
        let s1 = write_dedup(src.path(), &names, &idx1).unwrap();
        assert_eq!((s1.chunks, s1.new_chunks), (4, 4));

        // change a few bytes in the middle chunk only
        let mut changed = big.clone();
        changed[CHUNK_SIZE + 10] ^= 0xff;
        std::fs::write(src.path().join("disk-0.img"), &changed).unwrap();
        let idx2 = store.path().join("two.hxidx");
        let s2 = write_dedup(src.path(), &names, &idx2).unwrap();
        assert_eq!(s2.chunks, 4);
        assert_eq!(s2.new_chunks, 1, "only the changed chunk is written");
        assert!(s2.new_bytes < s1.new_bytes);
        assert_eq!(chunk_files(store.path()), 5);

        // both restore byte for byte
        for (idx, want) in [(&idx1, &big), (&idx2, &changed)] {
            let out = tempfile::tempdir().unwrap();
            extract_dedup(idx, out.path()).unwrap();
            assert_eq!(&std::fs::read(out.path().join("disk-0.img")).unwrap(), want);
            assert_eq!(
                std::fs::read(out.path().join("manifest.json")).unwrap(),
                b"{\"a\":1}"
            );
            assert_eq!(verify_chunks(idx).unwrap(), 4);
        }
    }

    #[test]
    fn damage_is_detected_and_gc_keeps_shared_chunks() {
        let src = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("f"), data(CHUNK_SIZE + 5, 7)).unwrap();
        let names = vec!["f".to_string()];
        let a = store.path().join("a.hxidx");
        let b = store.path().join("b.hxidx");
        write_dedup(src.path(), &names, &a).unwrap();
        write_dedup(src.path(), &names, &b).unwrap();
        assert_eq!(chunk_files(store.path()), 2);

        // deleting one backup keeps what the other needs
        std::fs::remove_file(&a).unwrap();
        assert_eq!(gc(store.path(), std::time::Duration::ZERO).unwrap().0, 0);
        assert_eq!(verify_chunks(&b).unwrap(), 2);

        // a flipped byte in a chunk is found by verify and by extract
        let idx = read_index(&b).unwrap();
        let victim = chunk_path(&store.path().join(CHUNK_DIR), &idx.files[0].chunks[0]);
        let mut packed = std::fs::read(&victim).unwrap();
        let mid = packed.len() / 2;
        packed[mid] ^= 0x55;
        std::fs::write(&victim, packed).unwrap();
        assert!(verify_chunks(&b)
            .unwrap_err()
            .to_string()
            .contains("corrupted"));
        let out = tempfile::tempdir().unwrap();
        assert!(extract_dedup(&b, out.path()).is_err());
        // a missing chunk too
        std::fs::remove_file(&victim).unwrap();
        assert!(verify_chunks(&b)
            .unwrap_err()
            .to_string()
            .contains("missing"));

        // deleting the last backup frees everything (a young chunk survives the grace period)
        std::fs::remove_file(&b).unwrap();
        let (n, _) = gc(store.path(), std::time::Duration::from_secs(3600)).unwrap();
        assert_eq!(n, 0, "young chunks are kept");
        let (n, freed) = gc(store.path(), std::time::Duration::ZERO).unwrap();
        assert_eq!(n, 1);
        assert!(freed > 0);
        assert!(!store.path().join(CHUNK_DIR).exists());
    }

    #[test]
    fn index_names_and_ids_are_validated() {
        let store = tempfile::tempdir().unwrap();
        let idx = store.path().join("x.hxidx");
        std::fs::write(
            &idx,
            r#"{"version":1,"chunk_size":4194304,"files":[{"name":"../evil","size":0,"chunks":[]}]}"#,
        )
        .unwrap();
        let out = tempfile::tempdir().unwrap();
        assert!(extract_dedup(&idx, out.path())
            .unwrap_err()
            .to_string()
            .contains("invalid file name"));
        std::fs::write(
            &idx,
            r#"{"version":1,"chunk_size":4194304,"files":[{"name":"f","size":1,"chunks":["../../etc/passwd"]}]}"#,
        )
        .unwrap();
        assert!(extract_dedup(&idx, out.path())
            .unwrap_err()
            .to_string()
            .contains("invalid chunk id"));
        std::fs::write(&idx, r#"{"version":9,"chunk_size":1,"files":[]}"#).unwrap();
        assert!(verify_chunks(&idx)
            .unwrap_err()
            .to_string()
            .contains("newer"));
    }
}
