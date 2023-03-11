use widestring::WideCString;
use windows::{
    core::{Error, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, ERROR_INVALID_PARAMETER, HANDLE},
        System::Threading::{CreateMutexW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE},
    },
};

type Result<R> = core::result::Result<R, Error>;

pub struct GlobalMutex(HANDLE);

#[allow(dead_code)]
impl GlobalMutex {
    pub fn create(name: &str, take_ownership: bool) -> Result<GlobalMutex> {
        let name = WideCString::from_str(name).map_err(|_| ERROR_INVALID_PARAMETER.to_hresult())?;

        let handle =
            unsafe { CreateMutexW(None, take_ownership, PCWSTR::from_raw(name.as_ptr()))? };

        // If the handle already exists, the function will set the last error to ERROR_ALREADY_EXISTS
        // and open a new handle reference.
        if Error::from_win32().code() == ERROR_ALREADY_EXISTS.to_hresult() {
            unsafe {
                CloseHandle(handle);
            }

            Err(Error::from_win32())?;
        }

        Ok(GlobalMutex(handle))
    }

    pub fn open(name: &str) -> Result<GlobalMutex> {
        let name = WideCString::from_str(name).map_err(|_| ERROR_INVALID_PARAMETER.to_hresult())?;

        Ok(GlobalMutex(unsafe {
            OpenMutexW(
                SYNCHRONIZATION_SYNCHRONIZE,
                false,
                PCWSTR::from_raw(name.as_ptr()),
            )?
        }))
    }
}

impl std::ops::Drop for GlobalMutex {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
