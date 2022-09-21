use windows::{
    core::{Error, PWSTR},
    Win32::{
        Foundation::HANDLE,
        System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_FORMAT,
        },
    },
};

type Result<R> = core::result::Result<R, Error>;

pub trait ProcessExt {
    fn full_image_name(&self) -> Result<String>;
}

impl ProcessExt for HANDLE {
    fn full_image_name(&self) -> Result<String> {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;

        match unsafe {
            QueryFullProcessImageNameW(
                self.clone(),
                PROCESS_NAME_FORMAT(0),
                PWSTR(&mut name as *mut u16),
                &mut len,
            )
            .as_bool()
        } {
            true => Ok(String::from_utf16(&name[..len as usize]).unwrap()),
            false => Err(Error::from_win32()),
        }
    }
}

pub fn open(access: u32, id: u32) -> Result<HANDLE> {
    unsafe { OpenProcess(PROCESS_ACCESS_RIGHTS(access), false, id) }
}
