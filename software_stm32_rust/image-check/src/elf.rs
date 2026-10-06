//! The part of an ELF32 little-endian file the checks need: section headers, the symbol table
//! and the bytes at a virtual address.

pub const SHT_PROGBITS: u32 = 1;
pub const SHT_SYMTAB: u32 = 2;
pub const SHF_ALLOC: u32 = 0x2;
pub const STT_FUNC: u8 = 2;

#[derive(Clone, Debug)]
pub struct Section {
    pub name: String,
    pub kind: u32,
    pub flags: u32,
    pub addr: u32,
    pub offset: u32,
    pub size: u32,
}

impl Section {
    pub fn end(&self) -> u32 {
        self.addr.wrapping_add(self.size)
    }

    pub fn contains(&self, addr: u32) -> bool {
        addr >= self.addr && addr < self.end()
    }
}

#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub value: u32,
    pub size: u32,
    pub kind: u8,
}

pub struct Elf {
    data: Vec<u8>,
    pub sections: Vec<Section>,
    pub symbols: Vec<Symbol>,
}

fn u16_at(d: &[u8], at: usize) -> Result<u16, String> {
    d.get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| format!("ELF truncated at {at:#x}"))
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, String> {
    d.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| format!("ELF truncated at {at:#x}"))
}

fn c_str(d: &[u8], at: usize) -> String {
    let tail = d.get(at..).unwrap_or_default();
    let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
    String::from_utf8_lossy(&tail[..end]).into_owned()
}

impl Elf {
    pub fn parse(data: Vec<u8>) -> Result<Elf, String> {
        if data.get(..4) != Some(b"\x7fELF".as_slice()) || data.get(4) != Some(&1) || data.get(5) != Some(&1) {
            return Err("not an ELF32 little-endian file".into());
        }
        let shoff = u32_at(&data, 0x20)? as usize;
        let shentsize = u16_at(&data, 0x2E)? as usize;
        let shnum = u16_at(&data, 0x30)? as usize;
        let shstrndx = u16_at(&data, 0x32)? as usize;
        let mut raw = Vec::with_capacity(shnum);
        for i in 0..shnum {
            let h = shoff + i * shentsize;
            raw.push((
                u32_at(&data, h)?,
                u32_at(&data, h + 4)?,
                u32_at(&data, h + 8)?,
                u32_at(&data, h + 12)?,
                u32_at(&data, h + 16)?,
                u32_at(&data, h + 20)?,
                u32_at(&data, h + 24)?,
            ));
        }
        let strtab_off = raw.get(shstrndx).map(|r| r.4 as usize).ok_or("no section names")?;
        let sections: Vec<Section> = raw
            .iter()
            .map(|&(name, kind, flags, addr, offset, size, _)| Section {
                name: c_str(&data, strtab_off + name as usize),
                kind,
                flags,
                addr,
                offset,
                size,
            })
            .collect();
        let mut symbols = Vec::new();
        for (i, s) in sections.iter().enumerate() {
            if s.kind != SHT_SYMTAB {
                continue;
            }
            let link = raw[i].6 as usize;
            let names = sections.get(link).ok_or("symtab without string table")?.offset as usize;
            for k in 0..(s.size as usize / 16) {
                let e = s.offset as usize + k * 16;
                let info = *data.get(e + 12).ok_or("symtab truncated")?;
                symbols.push(Symbol {
                    name: c_str(&data, names + u32_at(&data, e)? as usize),
                    value: u32_at(&data, e + 4)?,
                    size: u32_at(&data, e + 8)?,
                    kind: info & 0xF,
                });
            }
        }
        Ok(Elf {
            data,
            sections,
            symbols,
        })
    }

    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    pub fn symbol(&self, name: &str) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.name == name)
    }

    /// Allocated sections with content in the file (flash images).
    pub fn loaded(&self) -> impl Iterator<Item = &Section> {
        self.sections
            .iter()
            .filter(|s| s.kind == SHT_PROGBITS && s.flags & SHF_ALLOC != 0)
    }

    /// `len` bytes at virtual address `addr` from a PROGBITS section.
    pub fn bytes(&self, addr: u32, len: u32) -> Option<&[u8]> {
        let s = self
            .loaded()
            .find(|s| s.contains(addr) && addr.checked_add(len).is_some_and(|e| e <= s.end()))?;
        let at = (s.offset + (addr - s.addr)) as usize;
        self.data.get(at..at + len as usize)
    }

    pub fn u16(&self, addr: u32) -> Option<u16> {
        self.bytes(addr, 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&self, addr: u32) -> Option<u32> {
        self.bytes(addr, 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}
