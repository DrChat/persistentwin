use windows::{
    core::{Error, PWSTR},
    Win32::{
        Foundation::{ERROR_INTERNAL_ERROR, HANDLE},
        Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
        System::Threading::{
            OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_ACCESS_RIGHTS,
            PROCESS_NAME_FORMAT,
        },
    },
};

type Result<R> = core::result::Result<R, Error>;

pub trait ProcessExt {
    fn full_image_name(&self) -> Result<String>;
    fn is_elevated(&self) -> Result<bool>;
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

    fn is_elevated(&self) -> Result<bool> {
        let mut token = HANDLE::default();
        let mut elevation = TOKEN_ELEVATION::default();
        let mut ret_len = 0u32;

        match unsafe { OpenProcessToken(self.clone(), TOKEN_QUERY, &mut token).as_bool() } {
            true => {}
            false => {
                Err(Error::from_win32())?;
            }
        }

        match unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elevation as *mut _ as *mut _),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut ret_len,
            )
            .as_bool()
        } {
            true => {
                // Ensure the return length is correct.
                if ret_len != std::mem::size_of::<TOKEN_ELEVATION>() as u32 {
                    Err(ERROR_INTERNAL_ERROR.to_hresult())?
                }
            }
            false => {
                Err(Error::from_win32())?;
            }
        }

        Ok(elevation.TokenIsElevated != 0)
    }
}

pub fn open(access: u32, id: u32) -> Result<HANDLE> {
    unsafe { OpenProcess(PROCESS_ACCESS_RIGHTS(access), false, id) }
}
