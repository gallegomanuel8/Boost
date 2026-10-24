//! Positional file I/O on both platforms: every connection writes at its own
//! offset of the one partial file, without sharing a cursor.

use std::fs::File;
use std::io;

#[cfg(unix)]
pub fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}

#[cfg(unix)]
pub fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

/// `seek_write` may write less than asked and moves the shared cursor, which
/// positional writers never rely on.
#[cfg(windows)]
pub fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_write(buf, offset) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "failed to write whole buffer")),
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(windows)]
pub fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_order_writes() {
        let path = std::env::temp_dir().join(format!("raudo-fileio-{}", std::process::id()));
        let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
        write_all_at(&f, b"world", 6).unwrap();
        write_all_at(&f, b"hello ", 0).unwrap();
        let mut buf = [0u8; 11];
        assert_eq!(read_at(&f, &mut buf, 0).unwrap(), 11);
        assert_eq!(&buf, b"hello world");
        std::fs::remove_file(path).unwrap();
    }
}
