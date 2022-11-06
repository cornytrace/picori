//! Parse and build `.dol` files.
//!
//! ## Parse
//!
//! To parse a `.dol` file, use [`from_bytes`]. The section data is referenced
//! from the bytes passed to [`from_bytes`], thus ['Dol'] struct is only valid
//! for as long as those bytes are available.
//!
//! First we need to have access to the `.dol` file bytes. E.g., we can read
//! the file into a `Vec<u8>` using [`std::fs::read`]. Next step is to parse
//! the bytes into a [`Dol`] struct using [`from_bytes`]. If no error occurs,
//! we can access the sections using the [`sections`] field or the entrypoint
//! using the [`entrypoint`] method.
//! ```no_run
//! use anyhow::Result;
//! use std::fs::File;
//! fn main() -> Result<()> {
//!     let mut file = File::open("../../assets/gzle01.dol")?;
//!     let dol = picori::format::dol::from_bytes(&mut file)?;
//!     println!("entry point: {:#08x}", dol.entry_point());
//!     Ok(())
//! }
//! ```
//!
//! ## Build
//!
//! TODO: Write this section.

use std::io::{Seek, SeekFrom};
use std::result::Result;

use itertools::{chain, izip};

use crate::error::DolError; 
use crate::helper::{align_next, ReadExtension, ReadExtensionU32, SliceReader, TakeLastN};
use crate::DeserializeError;

/// The `.dol` header without any modifications. This is the raw data that is
/// read from the file. The data has been endian-flipped to be in the native
/// endian format.
#[derive(Debug)]
pub struct Header {
    pub text_offset:  [u32; 7],  // 0x00
    pub data_offset:  [u32; 11], // 0x1C
    pub text_address: [u32; 7],  // 0x48
    pub data_address: [u32; 11], // 0x64
    pub text_size:    [u32; 7],  // 0x90
    pub data_size:    [u32; 11], // 0xAC
    pub bss_address:  u32,       // 0xD8
    pub bss_size:     u32,       // 0xDC
    pub entry_point:  u32,       // 0xE0
}

/// Kind of a section in a `.dol` file.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SectionKind {
    /// Text section, e.g. `.init`, `.text`, etc.
    Text,

    // Data section, e.g. `extab_`, `extabindex_`, `.ctors`, `.dtors`, `.rodata`, `.data`,
    // `.sdata`, `.sdata2`, etc.
    Data,

    // BSS section, e.g., `.bss`, `.sbss`, `.sbss2`, etc.
    Bss,
}

/// A section in a `.dol` file.
#[derive(Debug)]
pub struct Section {
    /// The kind of section this is (text, data, or bss).
    pub kind: SectionKind,

    /// The section name (e.g. `.text`, `.data`, `.rodata`, etc.), this was
    /// guessed from the type of section and order in which they appear in
    /// the `.dol`. This is not guaranteed to be correct, as the `.dol`
    /// format does not specify the name of the section.
    pub name: &'static str,

    /// The section address that the data is loaded to in memory on startup.
    pub address: u32,

    /// The section size in bytes.
    pub size: u32,

    /// The section size in bytes, rounded up to the nearest multiple of 32.
    pub aligned_size: u32,

    /// The section data. For `.bss` sections ([`SectionKind::Bss`]), this
    /// will be an empty vector.
    pub data: Vec<u8>,
}

/// RomCopyInfo represents one entry in the `__rom_copy_info` symbol generated
/// by the linker at the end of the `.init` section. It has information
/// otherwise lost in the process of converting `.elf` to `.dol`, such as, the
/// original unaligned section size. At startup the `__rom_copy_info` is used to
/// copy each entry from the ROM to the RAM.
#[derive(Debug)]
pub struct RomCopyInfo {
    /// Read Only Memory (ROM) address of the section.
    pub rom_address: u32,

    /// Random Access Memory (RAM) address of the section.
    pub ram_address: u32,

    /// The size of the section in bytes.
    pub size: u32,
}

/// BssInitInfo represents one entry in the `__bss_init_info` symbol generated
/// by the linker at the end of the `.init` section. It has information
/// otherwise lost in the process of converting `.elf` to `.dol`, such as, the
/// original unaligned section size and how many `.bss` (`.sbss`, `.bss2`, etc.)
/// sections exists. The final `.dol` file will have a single `.bss` section
/// with the size of the sum of all the `.bss` sections. At startup the
/// `__bss_init_info` is used to zero out the `.bss` section in RAM.
#[derive(Debug)]
pub struct BssInitInfo {
    /// Random Access Memory (RAM) address of the section.
    pub ram_address: u32,

    /// The size of the section in bytes.
    pub size: u32,
}

pub struct Dol {
    pub header:        Header,
    pub rom_copy_info: Option<Vec<RomCopyInfo>>,
    pub bss_init_info: Option<Vec<BssInitInfo>>,
    pub sections:      Vec<Section>,
}

impl RomCopyInfo {
    fn from_bytes<Reader>(reader: &mut Reader) -> Result<Self, DeserializeError>
    where
        Reader: ReadExtension,
    {
        let rom_copy_info: Result<_, DeserializeError> = {
            let rom_address = reader.read_bu32()?;
            let ram_address = reader.read_bu32()?;
            let size = reader.read_bu32()?;
            println!("rom {rom_address} {ram_address} {size}");
            Ok(RomCopyInfo {
                rom_address,
                ram_address,
                size,
            })
        };

        if let Err(e) = rom_copy_info {
            Err(DeserializeError::InvalidData("invalid RomCopyInfo"))
        } else {
            Ok(rom_copy_info.unwrap())
        }
    }
}

impl BssInitInfo {
    fn from_bytes<Reader>(reader: &mut Reader) -> Result<Self, DeserializeError>
    where
        Reader: ReadExtension,
    {
        let rom_copy_info: Result<_, DeserializeError> = {
            let ram_address = reader.read_bu32()?;
            let size = reader.read_bu32()?;
            println!("bss {ram_address} {size}");
            Ok(BssInitInfo { ram_address, size })
        };

        if let Err(e) = rom_copy_info {
            Err(DeserializeError::InvalidData("invalid BssInitInfo"))
        } else {
            Ok(rom_copy_info.unwrap())
        }
    }
}

/// Search 0x200 bytes from the end of `data` (from the `.init` section)
/// until we find all `__rom_copy_info` entries.
fn rom_copy_info_search(data: &[u8], address: u32) -> Option<Vec<RomCopyInfo>> {
    Some(
        data.take_last_n(0x200)
            .windows(12)
            .map(|x| RomCopyInfo::from_bytes(&mut SliceReader::new(x)))
            .filter(|x| x.is_ok())
            .map(|x| x.unwrap())
            .skip_while(|x| x.rom_address != address || x.ram_address != address)
            .step_by(12)
            .take_while(|x| x.rom_address != 0)
            .collect(),
    )
}

/// Search 0x200 bytes from the end of `data` (from the `.init` section)
/// until we find all `__bss_init_info` entries.
fn bss_init_info_search(data: &[u8], address: u32) -> Option<Vec<BssInitInfo>> {
    Some(
        data.take_last_n(0x200)
            .windows(8)
            .map(|x| BssInitInfo::from_bytes(&mut SliceReader::new(x)))
            .filter(|x| x.is_ok())
            .map(|x| x.unwrap())
            .skip_while(|x| x.ram_address != address)
            .step_by(8)
            .take_while(|x| x.ram_address != 0)
            .collect(),
    )
}

fn section_name(kind: SectionKind, index: usize) -> &'static str {
    match kind {
        SectionKind::Text => match index {
            0 => ".init",
            1 => ".text",
            2 => ".text.2",
            3 => ".text.3",
            4 => ".text.4",
            5 => ".text.5",
            6 => ".text.6",
            _ => panic!("invalid text section index"),
        },
        SectionKind::Data => match index {
            0 => "extab_",
            1 => "extabindex_",
            2 => ".ctors",
            3 => ".dtors",
            4 => ".rodata",
            5 => ".data",
            6 => ".sdata",
            7 => ".sdata2",
            8 => ".data8",
            9 => ".data9",
            10 => ".data10",
            _ => panic!("invalid data section index"),
        },
        SectionKind::Bss => match index {
            0 => ".bss",
            1 => ".sbss",
            2 => ".sbss2",
            _ => panic!("invalid bss section index"),
        },
    }
}

impl Section {
    fn new<Reader>(
        reader: &mut Reader,
        kind: SectionKind,
        index: usize,
        offset: u32,
        address: u32,
        size: u32,
        aligned_size: u32,
    ) -> Result<Self, DeserializeError>
    where
        Reader: ReadExtension + Seek,
    {
        let mut data = unsafe {
            let mut data = Vec::with_capacity(size as usize);
            data.set_len(size as usize);
            data
        };

        reader.seek(SeekFrom::Start(offset as u64))?;
        reader.read_exact(data.as_mut_slice())?;

        Ok(Self {
            kind:         kind,
            name:         section_name(kind, index),
            address:      address,
            size:         size,
            aligned_size: aligned_size,
            data:         data,
        })
    }
}

/// Parse a `.dol` file and return a [`Dol`] struct on success. The
/// [`Dol`] struct contains all the information from the `.dol` file.
/// Additional information included is `__rom_copy_info` and
/// `__bss_init_info` if they are available.
pub fn from_bytes<Reader>(reader: &mut Reader) -> Result<Dol, DeserializeError>
where
    Reader: ReadExtension + Seek,
{
    let text_offset = reader.read_bu32_array::<7>()?;
    let data_offset = reader.read_bu32_array::<11>()?;
    let text_address = reader.read_bu32_array::<7>()?;
    let data_address = reader.read_bu32_array::<11>()?;
    let text_size = reader.read_bu32_array::<7>()?;
    let data_size = reader.read_bu32_array::<11>()?;
    let bss_address = reader.read_bu32()?;
    let bss_size = reader.read_bu32()?;
    let entry_point = reader.read_bu32()?;

    let text_sections = izip!(text_offset.iter(), text_address.iter(), text_size.iter());
    let text_sections = text_sections
        .enumerate()
        .map(|(i, x)| (SectionKind::Text, i, x));

    let data_sections = izip!(data_offset.iter(), data_address.iter(), data_size.iter());
    let data_sections = data_sections
        .enumerate()
        .map(|(i, x)| (SectionKind::Data, i, x));

    let mut sections: Vec<Section> = chain!(text_sections, data_sections)
        .map(|(kind, index, (offset, address, size))| {
            Section::new(reader, kind, index, *offset, *address, *size, *size)
        })
        .filter(|section| match section {
            Ok(section) => section.size != 0,
            Err(_) => true, // don't skip errors here, we want to propagate them to the caller
        })
        .collect::<Result<Vec<_>, _>>()?;

    let init = sections.iter().find(|x| x.name == ".init");
    let rom_copy_info = init.map_or(None, |init| {
        rom_copy_info_search(init.data.as_slice(), init.address)
    });
    let bss_init_info = init.map_or(None, |init| {
        bss_init_info_search(init.data.as_slice(), bss_address)
    });

    for section in sections.iter_mut() {
        section.size = rom_copy_info
            .as_ref()
            .and_then(|v| v.iter().find(|x| x.rom_address == section.address))
            .map_or(section.size, |x| x.size);
    }

    // If `__bss_init_info` is available we can use it to determine the size and
    // count of the `.bss` sections. Otherwise we assume that there is only one
    // `.bss` section and use the size from the header (which is probably
    // not correct).
    if let Some(bss_init_info) = &bss_init_info {
        let bss_sections = bss_init_info
            .iter()
            .enumerate()
            .map(|(index, entry)| Section {
                kind:         SectionKind::Bss,
                name:         section_name(SectionKind::Bss, index),
                address:      entry.ram_address,
                size:         entry.size,
                aligned_size: align_next(entry.size, 32),
                data:         vec![],
            });
        sections.extend(bss_sections)
    } else {
        // TODO: We can probably use the data section to determine the .bss sections.
        sections.push(Section {
            kind:         SectionKind::Bss,
            name:         section_name(SectionKind::Bss, 0),
            address:      bss_address,
            size:         bss_size,
            aligned_size: bss_size,
            data:         vec![],
        });
    }

    Ok(Dol {
        header:        Header {
            text_offset,
            data_offset,
            text_address,
            data_address,
            text_size,
            data_size,
            bss_address,
            bss_size,
            entry_point,
        },
        rom_copy_info: rom_copy_info,
        bss_init_info: bss_init_info,
        sections:      sections,
    })
}

pub fn to_bytes(_dol: &Dol) -> Result<Vec<u8>, DeserializeError> {
    unimplemented!("picori::format::dol::to_bytes");
}

impl Dol {
    /// Returns the entry point of the DOL file. This is the address of the
    /// first instruction that will be executed. The section containing the
    /// entry point can be found using [`Dol::section_by_address`]. Entry point
    /// is also available via direct access to the [`Dol::header`].
    #[inline]
    pub fn entry_point(&self) -> u32 { self.header.entry_point }

    /// Returns an [`Some(&Section)`] if the DOL file contains a section with
    /// the given name `name` or [`None`] otherwise. Section names are not
    /// information provided by the `.dol` format, instead we assign names
    /// based on the section kind [`SectionKind`] and the index of the section.
    #[inline]
    pub fn section_by_name(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|x| x.name == name)
    }

    /// Returns an [`Some(&Section)`] if the DOL file contains a section with
    /// that contains the given address `address` or [`None`] otherwise.
    #[inline]
    pub fn section_by_address(&self, address: u32) -> Option<&Section> {
        self.sections
            .iter()
            .find(|x| address >= x.address && address < x.address + x.size)
    }

    /// Parse a `.dol` file and return a [`Dol`] struct on success. This is a
    /// convenience function, it is equivalent to calling [`Dol::from_bytes`]
    /// with similar arguments.
    #[inline]
    pub fn from_bytes<Reader>(reader: &mut Reader) -> Result<Dol, DeserializeError>
    where
        Reader: ReadExtension + Seek,
    {
        from_bytes(reader)
    }
}
