#![cfg(target_os = "windows")]

use windows::{
    core::{HSTRING, PCWSTR},
    Win32::{
        Foundation::{FreeLibrary, HMODULE},
        System::LibraryLoader::{
            FindResourceW, LoadLibraryExW, LoadResource, LockResource, SizeofResource,
            LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        },
        UI::WindowsAndMessaging::{RT_GROUP_ICON, RT_ICON},
    },
};

fn resource(module: HMODULE, kind: PCWSTR, id: u16) -> Vec<u8> {
    unsafe {
        let info = FindResourceW(Some(module), PCWSTR(id as usize as *const u16), kind);
        assert!(!info.is_invalid(), "missing resource {id}");
        let size = SizeofResource(Some(module), info) as usize;
        assert!(size > 0);
        let data = LoadResource(Some(module), info).unwrap();
        let bytes = LockResource(data).cast::<u8>();
        assert!(!bytes.is_null());
        std::slice::from_raw_parts(bytes, size).to_vec()
    }
}

#[test]
fn executable_embeds_brand_icon_at_gpui_resource_id() {
    let icon = include_bytes!("../../../brand/app-icon/export/lagom.ico");
    // Map resources only: no executable code runs and no window is created.
    let module = unsafe {
        LoadLibraryExW(
            &HSTRING::from(env!("CARGO_BIN_EXE_lagom")),
            None,
            LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        ).unwrap()
    };
    let group = resource(module, RT_GROUP_ICON, 1);
    assert_eq!(&group[..6], &icon[..6]);
    let count = u16::from_le_bytes(icon[4..6].try_into().unwrap()) as usize;
    assert!(count > 0);
    assert_eq!(group.len(), 6 + count * 14);
    for i in 0..count {
        let source = &icon[6 + i * 16..6 + (i + 1) * 16];
        let entry = &group[6 + i * 14..6 + (i + 1) * 14];
        assert_eq!(&entry[..12], &source[..12]);
        let id = u16::from_le_bytes(entry[12..14].try_into().unwrap());
        let size = u32::from_le_bytes(source[8..12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(source[12..16].try_into().unwrap()) as usize;
        assert_eq!(resource(module, RT_ICON, id), &icon[offset..offset + size]);
    }
    unsafe { FreeLibrary(module).unwrap() };
}
