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
    pub signed: bool,
    pub gui: Part,
    pub webview: Part,
    pub payload: Part,
}

pub fn open(path: &Path) -> io::Result<(File, Package)> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let (end, signed) = package_end(&mut file, size)?;
    let (footer, footer_end) = read_footer(&mut file, end, signed)?;
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
        signed,
        gui: pieces.next().unwrap(),
        webview: pieces.next().unwrap(),
        payload: pieces.next().unwrap(),
    };
    if position.checked_add(FOOTER) != Some(footer_end)
        || [package.stub, package.gui, package.webview, package.payload]
            .iter()
            .any(|part| part.len == 0)
    {
        return Err(invalid());
    }
    Ok((file, package))
}

// The PE Security Directory is a file offset, unlike other PE directories.
fn package_end(file: &mut File, size: u64) -> io::Result<(u64, bool)> {
    if size < FOOTER {
        return Err(invalid());
    }
    let mut magic = [0; 2];
    file.read_exact(&mut magic)?;
    if &magic != b"MZ" {
        return Ok((size, false));
    }

    file.seek(SeekFrom::Start(0x3c))?;
    let pe = u64::from(read_u32(file)?);
    if pe < 0x40 || pe + 24 > size {
        return Err(invalid());
    }
    file.seek(SeekFrom::Start(pe))?;
    let mut header = [0; 24];
    file.read_exact(&mut header)?;
    if &header[..4] != b"PE\0\0" {
        return Err(invalid());
    }

    let optional = pe + 24;
    let optional_size = u64::from(u16::from_le_bytes([header[20], header[21]]));
    file.read_exact(&mut magic)?;
    let directories = match u16::from_le_bytes(magic) {
        0x10b => 96,
        0x20b => 112,
        _ => return Err(invalid()),
    };
    if optional_size < directories || optional + optional_size > size {
        return Err(invalid());
    }
    file.seek(SeekFrom::Start(optional + directories - 4))?;
    if read_u32(file)? < 5 {
        return Ok((size, false));
    }
    let security_entry = directories + 4 * 8;
    if optional_size < security_entry + 8 {
        return Err(invalid());
    }
    file.seek(SeekFrom::Start(optional + security_entry))?;
    let start = u64::from(read_u32(file)?);
    let length = u64::from(read_u32(file)?);
    if start == 0 && length == 0 {
        return Ok((size, false));
    }
    if start < optional + optional_size + FOOTER
        || start % 8 != 0
        || length < 8
        || length % 8 != 0
        || start + length != size
    {
        return Err(invalid());
    }
    Ok((start, true))
}

fn read_u32(file: &mut File) -> io::Result<u32> {
    let mut bytes = [0; 4];
    file.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_footer(
    file: &mut File,
    end: u64,
    signed: bool,
) -> io::Result<([u8; FOOTER as usize], u64)> {
    // SignTool can insert up to seven zero bytes before the certificate table.
    let max_padding = if signed { 7 } else { 0 };
    let mut footer = [0; FOOTER as usize];
    for padding in 0..=max_padding {
        let start = end.checked_sub(FOOTER + padding).ok_or_else(invalid)?;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut footer)?;
        if &footer[..8] == MAGIC {
            let mut pad = [0; 7];
            file.read_exact(&mut pad[..padding as usize])?;
            if pad.iter().all(|byte| *byte == 0) {
                return Ok((footer, end - padding));
            }
        }
    }
    Err(invalid())
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
    fn finds_footer_before_a_pe_certificate_table() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("setup.exe");
        let stub_len = 512_u64;
        let mut setup = vec![0_u8; stub_len as usize];
        setup[..2].copy_from_slice(b"MZ");
        setup[0x3c..0x40].copy_from_slice(&0x80_u32.to_le_bytes());
        setup[0x80..0x84].copy_from_slice(b"PE\0\0");
        setup[0x94..0x96].copy_from_slice(&240_u16.to_le_bytes());
        setup[0x98..0x9a].copy_from_slice(&0x20b_u16.to_le_bytes());
        setup[0x104..0x108].copy_from_slice(&16_u32.to_le_bytes());
        setup.extend_from_slice(b"guibootstrapzip");
        setup.extend_from_slice(MAGIC);
        for length in [stub_len, 3, 9, 3] {
            setup.extend_from_slice(&length.to_le_bytes());
        }

        std::fs::write(&path, &setup).unwrap();
        assert!(!open(&path).unwrap().1.signed);

        let footer_end = setup.len();
        let cert_start = footer_end.next_multiple_of(8);
        setup.resize(cert_start, 0);
        setup.extend_from_slice(&[16, 0, 0, 0, 0, 2, 2, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
        setup[0x128..0x12c].copy_from_slice(&(cert_start as u32).to_le_bytes());
        setup[0x12c..0x130].copy_from_slice(&16_u32.to_le_bytes());
        std::fs::write(&path, &setup).unwrap();
        let (mut file, package) = open(&path).unwrap();
        assert!(package.signed);
        let payload = root.path().join("payload");
        copy(&mut file, package.payload, &payload).unwrap();
        assert_eq!(std::fs::read(payload).unwrap(), b"zip");
        drop(file);

        setup[footer_end] = 1;
        std::fs::write(&path, &setup).unwrap();
        assert!(open(&path).is_err());
        setup[footer_end] = 0;
        setup[0x12c] = 24;
        std::fs::write(&path, &setup).unwrap();
        assert!(open(&path).is_err());
    }

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
