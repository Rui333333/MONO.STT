use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

const MAGIC: &[u8; 8] = b"MONOSTT1";
const FOOTER: u64 = 40;

#[derive(Clone, Copy)]
pub struct Part {
    pub offset: u64,
    pub len: u64,
}

pub struct Package {
    pub stub: Part,
    pub gui: Part,
    pub webview: Part,
    pub payload: Part,
}

pub fn open(path: &Path) -> io::Result<(File, Package)> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    if size < FOOTER {
        return Err(invalid());
    }
    file.seek(SeekFrom::End(-(FOOTER as i64)))?;
    let mut footer = [0; FOOTER as usize];
    file.read_exact(&mut footer)?;
    if &footer[..8] != MAGIC {
        return Err(invalid());
    }
    let mut position = 0_u64;
    let mut pieces = footer[8..].chunks_exact(8).map(|bytes| {
        let len = u64::from_le_bytes(bytes.try_into().unwrap());
        let part = Part {
            offset: position,
            len,
        };
        position = position.checked_add(len).unwrap_or(u64::MAX);
        part
    });
    let package = Package {
        stub: pieces.next().unwrap(),
        gui: pieces.next().unwrap(),
        webview: pieces.next().unwrap(),
        payload: pieces.next().unwrap(),
    };
    if position != size - FOOTER
        || [package.stub, package.gui, package.webview, package.payload]
            .iter()
            .any(|part| part.len == 0)
    {
        return Err(invalid());
    }
    Ok((file, package))
}

pub fn copy(file: &mut File, part: Part, dest: &Path) -> io::Result<()> {
    file.seek(SeekFrom::Start(part.offset))?;
    let mut output = File::create_new(dest)?;
    if io::copy(&mut file.take(part.len), &mut output)? != part.len {
        return Err(invalid());
    }
    output.flush()
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "安装包不完整或已经损坏")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_all_payload_boundaries() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("setup.exe");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"1234567890MONOSTT1").unwrap();
        for length in [1_u64, 2, 3, 4] {
            file.write_all(&length.to_le_bytes()).unwrap();
        }
        drop(file);
        let (mut file, package) = open(&path).unwrap();
        let payload = root.path().join("payload");
        copy(&mut file, package.payload, &payload).unwrap();
        assert_eq!(std::fs::read(payload).unwrap(), b"7890");
        drop(file);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(49)
            .unwrap();
        assert!(open(&path).is_err());
    }
}
