//! Provides detection and access to System Management BIOS (SMBIOS) and
//! Desktop Management Interface (DMI) data and structures.

extern crate bytes;

use bytes::Buf;
use std::fs;
use std::io;
use std::io::prelude::*;
use std::path;

/// Provides access to common information for SMBIOS entry points, including the
/// SMBIOS version in use and the location and size of the SMBIOS table in
/// system memory.
pub trait EntryPoint {
    /// Provides the address of the SMBIOS table in system memory and its size
    /// in bytes.
    fn table(&self) -> (usize, usize);

    /// Provides the major, minor, and revision numbers for SMBIOS on this
    /// system.
    fn version(&self) -> (usize, usize, usize);
}

/// Decodes an SMBIOS data stream from an input Read trait object.
pub struct Decoder<T: Read> {
    stream: io::BufReader<T>,
}

impl<T: Read> Decoder<T> {
    /// Creates a new Decoder by accepting an input stream with the Read trait.
    pub fn new(stream: T) -> Self {
        Decoder {
            stream: io::BufReader::new(stream),
        }
    }

    /// Decodes a vector of SMBIOS structures from the Decoder's stream.
    pub fn decode(&mut self) -> io::Result<Vec<Structure>> {
        let mut structures = Vec::new();

        // Header always occupies 4 bytes.
        let mut header_buf = [0; 4];
        loop {
            self.stream.read_exact(&mut header_buf)?;
            let header = parse_header(header_buf);

            // Formatted section is indicated length minus size of the header.
            let mut formatted = vec![0; header.length as usize - 4];
            self.stream.read_exact(&mut formatted)?;

            let strings = self.parse_strings()?;

            let header_type = header.header_type;

            structures.push(Structure {
                header,
                formatted,
                strings,
            });

            // Indicates end-of-structures in SMBIOS table.
            if header_type == 127 {
                return Ok(structures);
            }
        }
    }

    fn parse_strings(&mut self) -> io::Result<Vec<String>> {
        let mut strings = Vec::new();

        // It is possible for no strings to be presented; if so, two null bytes
        // will occur immediately and we return an empty vector.
        let mut prefix_buf = [0; 2];
        self.stream.read_exact(&mut prefix_buf)?;

        if prefix_buf == [0, 0] {
            return Ok(strings);
        }

        // Otherwise, keep looping and reading strings until we encounter two null bytes,
        // indicating end of strings.
        let mut upper = 2;
        loop {
            let string = self.parse_string(&mut prefix_buf[0..upper])?;
            strings.push(string);

            // From now on, we'll only use 1 byte of the prefix buffer.
            upper = 1;
            self.stream.read_exact(&mut prefix_buf[0..upper])?;

            // If we read a second null byte after parsing a string, end of
            // strings section.
            if prefix_buf[0] == 0 {
                return Ok(strings);
            }
        }
    }

    fn parse_string(&mut self, prefix: &mut [u8]) -> io::Result<String> {
        // Each string is terminated with a null byte.
        let mut buf = Vec::new();
        self.stream.read_until(0, &mut buf)?;

        // Remove the null byte from the string so it isn't parsed later.
        let i = buf.len() - 1;
        buf.remove(i);

        // Take the prefix buffer and append the string's bytes to get the
        // completed string.
        let mut string_vec = prefix.to_vec();
        string_vec.append(&mut buf);

        // TODO(mdlayher): don't unwrap, handle properly.
        Ok(String::from_utf8(string_vec).unwrap())
    }
}

// Predetermined locations where SMBIOS information can be found.
const LINUX_SYSFS_DMI: &str = "/sys/firmware/dmi/tables/DMI";
const LINUX_SYSFS_ENTRY_POINT: &str = "/sys/firmware/dmi/tables/smbios_entry_point";

/// Detects the entry point and location of an SMBIOS stream on this system,
/// returning the entry point found and a stream which can be used with the
/// Decoder type.
// TODO(mdlayher): is this signature idiomatic?  Should this function just
// decode the stream instead?
pub fn stream() -> io::Result<(EntryPointType, Box<Read>)> {
    // For now, we only support the standard Linux sysfs location.
    // TODO(mdlayher): read from /dev/mem as a fallback.
    if !path::Path::new(LINUX_SYSFS_ENTRY_POINT).exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "entry point not found",
        ));
    }

    let entry_point = fs::File::open(LINUX_SYSFS_ENTRY_POINT)?;
    let dmi = fs::File::open(LINUX_SYSFS_DMI)?;

    Ok((parse_entry_point(entry_point)?, Box::new(dmi)))
}

/// Indicates the type of data contained within an SMBIOS structure.
#[derive(Debug, PartialEq)]
pub struct Header {
    pub header_type: u8,
    pub length: u8,
    pub handle: u16,
}

fn parse_header(buf: [u8; 4]) -> Header {
    let mut cursor = io::Cursor::new(buf);
    Header {
        header_type: cursor.get_u8(),
        length: cursor.get_u8(),
        handle: cursor.get_u16_le(),
    }
}

/// Contains a single SMBIOS structure which can be interpreted using the SMBIOS
/// specification.
#[derive(Debug, PartialEq)]
pub struct Structure {
    pub header: Header,
    pub formatted: Vec<u8>,
    pub strings: Vec<String>,
}

fn parse_entry_point<T: Read>(mut stream: T) -> io::Result<EntryPointType> {
    // The entry point should be smaller than 64 bytes.
    let mut buf = [0; 64];
    let n = stream.read(&mut buf)?;

    Ok(match buf[0..5] {
        // 64-bit entry point.
        [b'_', b'S', b'M', b'3', b'_'] => EntryPointType::Bits64(parse_64bit(&buf[0..n])?),
        _ => EntryPointType::Unknown,
    })
}

/// Possible entry point types and their contained structures.
#[derive(Debug)]
pub enum EntryPointType {
    Unknown,        // Unknown entry point.
    Bits64(Bits64), // 64-bit entry point.
}

impl EntryPoint for Bits64 {
    fn table(&self) -> (usize, usize) {
        (
            self.structure_table_address as usize,
            self.structure_table_max_size as usize,
        )
    }

    fn version(&self) -> (usize, usize, usize) {
        (
            self.major as usize,
            self.minor as usize,
            self.revision as usize,
        )
    }
}

/// Contains the information found in a 64-bit SMBIOS entry point.
#[derive(Debug, PartialEq)]
pub struct Bits64 {
    checksum: u8,
    length: u8,
    major: u8,
    minor: u8,
    revision: u8,
    entry_point_revision: u8,
    reserved: u8,
    structure_table_max_size: u32,
    structure_table_address: u64,
}

fn parse_64bit(buf: &[u8]) -> io::Result<Bits64> {
    // Could potentially contain more data if we're reading from /dev/mem.
    if buf.len() < 24 {
        // TODO(mdlayher): our own error types.
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not enough data for 64-bit entry point",
        ));
    }

    let mut cursor = io::Cursor::new(buf);

    // Skip the anchor string.
    cursor.set_position(5);

    Ok(Bits64 {
        checksum: cursor.get_u8(),
        length: cursor.get_u8(),
        major: cursor.get_u8(),
        minor: cursor.get_u8(),
        revision: cursor.get_u8(),
        entry_point_revision: cursor.get_u8(),
        reserved: cursor.get_u8(),
        structure_table_max_size: cursor.get_u32_le(),
        structure_table_address: cursor.get_u64_le(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_point_64bit_ok() {
        let cursor = io::Cursor::new(vec![
            b'_', b'S', b'M', b'3', b'_', 0x86, 0x18, 0x3, 0x0, 0x0, 0x1, 0x0, 0x53, 0x9, 0x0, 0x0,
            0xb0, 0xb3, 0xe, 0x0, 0x0, 0x0, 0x0, 0x0,
        ]);

        let entry_point = parse_entry_point(cursor).expect("expected valid 64-bit entry point");

        match entry_point {
            EntryPointType::Bits64(got) => {
                let want = Bits64 {
                    checksum: 134,
                    length: 24,
                    major: 3,
                    minor: 0,
                    revision: 0,
                    entry_point_revision: 1,
                    reserved: 0,
                    structure_table_max_size: 2387,
                    structure_table_address: 963_504,
                };

                assert_eq!(want, got);
                assert_eq!((3, 0, 0), got.version());
                assert_eq!((963_504, 2387), got.table());
            }
            _ => panic!("invalid entry point type"),
        }
    }

    #[test]
    fn entry_point_64bit_bad() {
        let cursor = io::Cursor::new(vec![b'_', b'S', b'M', b'3', b'_', 0xff]);

        let _ = parse_entry_point(cursor).expect_err("expected invalid 64-bit entry point");
    }

    #[test]
    fn decode_structure_header_only_ok() {
        let got = unwrap_structure(vec![127, 0x04, 0x01, 0x00, 0x00, 0x00]);

        let want = Structure {
            header: Header {
                header_type: 127,
                length: 4,
                handle: 1,
            },
            formatted: vec![],
            strings: vec![],
        };

        assert_eq!(want, got);
    }

    #[test]
    fn decode_structure_no_strings_ok() {
        let got = unwrap_structure(vec![127, 0x06, 0x01, 0x00, 0x01, 0x02, 0x00, 0x00]);

        let want = Structure {
            header: Header {
                header_type: 127,
                length: 6,
                handle: 1,
            },
            formatted: vec![1, 2],
            strings: vec![],
        };

        assert_eq!(want, got);
    }

    #[test]
    fn decode_structure_all_ok() {
        let got = unwrap_structure(vec![
            127, 0x06, 0x01, 0x00, 0x01, 0x02, b'a', b'b', b'c', b'd', 0x00, b'1', b'2', b'3',
            b'4', 0x00, 0x00,
        ]);

        let want = Structure {
            header: Header {
                header_type: 127,
                length: 6,
                handle: 1,
            },
            formatted: vec![1, 2],
            strings: vec!["abcd".to_string(), "1234".to_string()],
        };

        assert_eq!(want, got);
    }

    #[test]
    fn decode_structure_multiple_ok() {
        let got = unwrap_structures(vec![
            0x00, 0x05, 0x01, 0x00, 0xff, 0x00, 0x00, 0x01, 0x0c, 0x02, 0x00, 0xde, 0xad, 0xbe,
            0xef, 0xde, 0xad, 0xbe, 0xef, b'd', b'e', b'a', b'd', b'b', b'e', b'e', b'f', 0x00,
            0x00, 127, 0x06, 0x03, 0x00, 0x01, 0x02, b'a', b'b', b'c', b'd', 0x00, b'1', b'2',
            b'3', b'4', 0x00, 0x00,
        ]);

        let want = vec![
            Structure {
                header: Header {
                    header_type: 0,
                    length: 5,
                    handle: 1,
                },
                formatted: vec![255],
                strings: vec![],
            },
            Structure {
                header: Header {
                    header_type: 1,
                    length: 12,
                    handle: 2,
                },
                formatted: vec![222, 173, 190, 239, 222, 173, 190, 239],
                strings: vec!["deadbeef".to_string()],
            },
            Structure {
                header: Header {
                    header_type: 127,
                    length: 6,
                    handle: 3,
                },
                formatted: vec![1, 2],
                strings: vec!["abcd".to_string(), "1234".to_string()],
            },
        ];

        assert_eq!(want, got);
    }

    fn unwrap_structure(buf: Vec<u8>) -> Structure {
        let mut structures = unwrap_structures(buf);
        if structures.len() != 1 {
            panic!("only expected one structure");
        }

        structures.pop().unwrap()
    }

    fn unwrap_structures(buf: Vec<u8>) -> Vec<Structure> {
        let cursor = io::Cursor::new(buf);

        let mut decoder = Decoder::new(cursor);

        decoder.decode().unwrap()
    }
}