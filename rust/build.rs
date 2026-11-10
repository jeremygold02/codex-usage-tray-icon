//! Embed the repository icon without a resource-compiler dependency.
//!
//! MSVC's linker accepts the Windows `.res` format directly. The GNU linker
//! needs a COFF conversion tool, so non-MSVC targets retain their existing
//! resource-free build rather than requiring another toolchain component.

use std::env;
use std::fs;
use std::io::{self, Error, ErrorKind};
use std::path::PathBuf;

fn invalid_icon() -> Error {
    Error::new(ErrorKind::InvalidData, "invalid icon.ico")
}

fn word(bytes: &[u8], offset: usize) -> io::Result<u16> {
    let value = bytes.get(offset..offset + 2).ok_or_else(invalid_icon)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn dword(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let value = bytes.get(offset..offset + 4).ok_or_else(invalid_icon)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn push_word(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn push_dword(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

/// Adds an ordinal-named resource, with a DWORD-aligned payload.
fn resource(output: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) -> io::Result<()> {
    let size = u32::try_from(data.len()).map_err(|_| invalid_icon())?;
    push_dword(output, size);
    push_dword(output, 32); // Ordinal type and name give a 32-byte header.
    push_word(output, 0xffff);
    push_word(output, kind);
    push_word(output, 0xffff);
    push_word(output, id);
    push_dword(output, 0); // Data version.
    push_word(output, 0x1030); // Moveable, shareable, discardable.
    push_word(output, 0); // Language-neutral.
    push_dword(output, 0); // Version.
    push_dword(output, 0); // Characteristics.
    output.extend_from_slice(data);
    while output.len() & 3 != 0 {
        output.push(0);
    }
    Ok(())
}

fn icon_resources(icon: &[u8]) -> io::Result<Vec<u8>> {
    if word(icon, 0)? != 0 || word(icon, 2)? != 1 {
        return Err(invalid_icon());
    }
    let count = word(icon, 4)?;
    if count == 0 {
        return Err(invalid_icon());
    }
    let directory_end = 6usize + usize::from(count) * 16;
    if icon.len() < directory_end {
        return Err(invalid_icon());
    }

    let mut result = Vec::new();
    // A 32-bit resource file begins with an empty resource header.
    result.extend_from_slice(&[
        0, 0, 0, 0, 32, 0, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0,
    ]);
    let mut group = Vec::with_capacity(6 + usize::from(count) * 14);
    push_word(&mut group, 0);
    push_word(&mut group, 1);
    push_word(&mut group, count);

    for index in 0..count {
        let entry = 6 + usize::from(index) * 16;
        let size = dword(icon, entry + 8)?;
        let offset = usize::try_from(dword(icon, entry + 12)?).map_err(|_| invalid_icon())?;
        let end = offset
            .checked_add(usize::try_from(size).map_err(|_| invalid_icon())?)
            .ok_or_else(invalid_icon)?;
        if offset < directory_end || size == 0 {
            return Err(invalid_icon());
        }
        let image = icon.get(offset..end).ok_or_else(invalid_icon)?;
        let id = index + 1;
        resource(&mut result, 3, id, image)?; // RT_ICON.
        group.extend_from_slice(&icon[entry..entry + 8]);
        push_dword(&mut group, size);
        push_word(&mut group, id);
    }
    resource(&mut result, 14, 1, &group)?; // RT_GROUP_ICON.
    Ok(result)
}

fn main() -> io::Result<()> {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return Ok(());
    }
    let manifest = env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| Error::new(ErrorKind::NotFound, "CARGO_MANIFEST_DIR is missing"))?;
    let icon_path = manifest.join("../icon.ico");
    println!("cargo:rerun-if-changed={}", icon_path.display());
    let output_dir = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| Error::new(ErrorKind::NotFound, "OUT_DIR is missing"))?;
    let output_path = output_dir.join("usage-tray-icon.res");
    fs::write(&output_path, icon_resources(&fs::read(icon_path)?)?)?;
    println!("cargo:rustc-link-arg={}", output_path.display());
    Ok(())
}
