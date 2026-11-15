//! Архитектура бинарника по заголовку: установка не должна запускать чужой
//! бинарник, поэтому заголовок читается до первого `-version`.

use std::io::Read;
use std::path::Path;

const X86_64: &str = "x86_64";
const AARCH64: &str = "aarch64";

const ELF_MACHINE: usize = 18;
const PE_OFFSET: usize = 0x3C;
const MACH_CPU: usize = 4;

pub fn binary_arch(path: &Path) -> Option<&'static str> {
    let mut head = Vec::with_capacity(1024);
    std::fs::File::open(path)
        .ok()?
        .take(1024)
        .read_to_end(&mut head)
        .ok()?;
    arch_of(&head)
}

fn arch_of(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"\x7fELF") {
        return match u16::from_le_bytes(head.get(ELF_MACHINE..ELF_MACHINE + 2)?.try_into().ok()?) {
            0x3E => Some(X86_64),
            0xB7 => Some(AARCH64),
            _ => None,
        };
    }
    if head.starts_with(b"MZ") {
        let offset =
            u32::from_le_bytes(head.get(PE_OFFSET..PE_OFFSET + 4)?.try_into().ok()?) as usize;
        if head.get(offset..offset + 4)? != b"PE\0\0" {
            return None;
        }
        return match u16::from_le_bytes(head.get(offset + 4..offset + 6)?.try_into().ok()?) {
            0x8664 => Some(X86_64),
            0xAA64 => Some(AARCH64),
            _ => None,
        };
    }
    if head.get(..4)? == [0xCF, 0xFA, 0xED, 0xFE] || head.get(..4)? == [0xCE, 0xFA, 0xED, 0xFE] {
        return mach_arch(u32::from_le_bytes(
            head.get(MACH_CPU..MACH_CPU + 4)?.try_into().ok()?,
        ));
    }
    if head.get(..4)? == [0xCA, 0xFE, 0xBA, 0xBE] || head.get(..4)? == [0xCA, 0xFE, 0xBA, 0xBF] {
        // Universal: arm64 предпочтителен, продукт собирается только под него.
        let count = u32::from_be_bytes(head.get(4..8)?.try_into().ok()?) as usize;
        let wide = head.get(..4)? == [0xCA, 0xFE, 0xBA, 0xBF];
        let stride = if wide { 32 } else { 20 };
        let mut found = None;
        for index in 0..count.min(16) {
            let at = 8 + index * stride;
            let cpu = u32::from_be_bytes(head.get(at..at + 4)?.try_into().ok()?);
            let Some(arch) = mach_arch(cpu) else {
                continue;
            };
            if arch == AARCH64 {
                return Some(AARCH64);
            }
            found = Some(arch);
        }
        return found;
    }
    None
}

fn mach_arch(cputype: u32) -> Option<&'static str> {
    match cputype {
        0x0100_0007 => Some(X86_64),
        0x0100_000C => Some(AARCH64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elf(machine: u16) -> Vec<u8> {
        let mut head = vec![0u8; 64];
        head[..4].copy_from_slice(b"\x7fELF");
        head[ELF_MACHINE..ELF_MACHINE + 2].copy_from_slice(&machine.to_le_bytes());
        head
    }

    fn macho(cputype: u32) -> Vec<u8> {
        let mut head = vec![0u8; 32];
        head[..4].copy_from_slice(&[0xCF, 0xFA, 0xED, 0xFE]);
        head[MACH_CPU..MACH_CPU + 4].copy_from_slice(&cputype.to_le_bytes());
        head
    }

    fn fat(cputypes: &[u32]) -> Vec<u8> {
        let mut head = vec![0u8; 8 + cputypes.len() * 20];
        head[..4].copy_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
        head[4..8].copy_from_slice(&(cputypes.len() as u32).to_be_bytes());
        for (index, cpu) in cputypes.iter().enumerate() {
            let at = 8 + index * 20;
            head[at..at + 4].copy_from_slice(&cpu.to_be_bytes());
        }
        head
    }

    fn pe(machine: u16) -> Vec<u8> {
        let mut head = vec![0u8; 256];
        head[..2].copy_from_slice(b"MZ");
        head[PE_OFFSET..PE_OFFSET + 4].copy_from_slice(&0x80u32.to_le_bytes());
        head[0x80..0x84].copy_from_slice(b"PE\0\0");
        head[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        head
    }

    #[test]
    fn elf_header_arch() {
        assert_eq!(arch_of(&elf(0x3E)), Some("x86_64"));
        assert_eq!(arch_of(&elf(0xB7)), Some("aarch64"));
        assert_eq!(arch_of(&elf(0x28)), None);
    }

    #[test]
    fn macho_header_arch() {
        assert_eq!(arch_of(&macho(0x0100_000C)), Some("aarch64"));
        assert_eq!(arch_of(&macho(0x0100_0007)), Some("x86_64"));
        assert_eq!(arch_of(&fat(&[0x0100_0007, 0x0100_000C])), Some("aarch64"));
        assert_eq!(arch_of(&fat(&[0x0100_0007])), Some("x86_64"));
        assert_eq!(arch_of(&macho(7)), None);
    }

    #[test]
    fn pe_header_arch() {
        assert_eq!(arch_of(&pe(0x8664)), Some("x86_64"));
        assert_eq!(arch_of(&pe(0xAA64)), Some("aarch64"));
        assert_eq!(arch_of(&pe(0x014C)), None);
    }

    #[test]
    fn unknown_and_truncated_headers() {
        assert_eq!(arch_of(b"#!/bin/sh\n"), None);
        assert_eq!(arch_of(b"MZ"), None);
        assert_eq!(arch_of(b"\x7fELF\x02"), None);
        assert_eq!(arch_of(&[]), None);
    }
}
