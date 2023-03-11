use windows::{
    core::Error,
    Win32::{
        Foundation::{BOOL, HWND, LPARAM},
        UI::WindowsAndMessaging::{
            EnumWindows, GetAncestor, GetClassNameW, GetWindowPlacement, GetWindowTextLengthW,
            GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, SetWindowPlacement, GA_ROOT,
            WINDOWPLACEMENT,
        },
    },
};

type Result<R> = core::result::Result<R, Error>;

pub struct OwnerInfo {
    pub process_id: u32,
    pub thread_id: u32,
}

pub trait HwndExt {
    fn class_name(&self) -> Result<String>;
    fn title(&self) -> Result<String>;
    fn placement(&self) -> Result<WINDOWPLACEMENT>;
    fn set_placement(&self, placement: WINDOWPLACEMENT) -> Result<()>;
    fn is_top_level(&self) -> bool;
    fn owner(&self) -> Result<OwnerInfo>;
    fn is_visible(&self) -> bool;
}

impl HwndExt for HWND {
    fn class_name(&self) -> Result<String> {
        let mut buf: Vec<u16> = Vec::new();
        buf.resize(256, 0u16);

        match unsafe { GetClassNameW(self.clone(), &mut buf) } {
            n if n > 0 => Ok(String::from_utf16(&buf[..n as usize]).unwrap()),
            _ => Err(Error::from_win32()),
        }
    }

    fn title(&self) -> Result<String> {
        let len = unsafe { GetWindowTextLengthW(self.clone()) };
        if len <= 0 {
            // Check if the title is just empty.
            let err = Error::from_win32();
            if err.code().0 == 0 {
                return Ok(String::new());
            }

            Err(err)?;
        }

        let mut buf = Vec::new();
        buf.resize((len + 1) as usize, 0u16);

        let len = unsafe { GetWindowTextW(self.clone(), &mut buf) };
        if len <= 0 {
            Err(Error::from_win32())?;
        }

        Ok(String::from_utf16(&buf).unwrap())
    }

    fn placement(&self) -> Result<WINDOWPLACEMENT> {
        let mut placement: WINDOWPLACEMENT = Default::default();
        placement.length = core::mem::size_of::<WINDOWPLACEMENT>() as u32;

        match unsafe { GetWindowPlacement(self.clone(), &mut placement) }.as_bool() {
            true => Ok(placement),
            false => Err(Error::from_win32()),
        }
    }

    fn set_placement(&self, placement: WINDOWPLACEMENT) -> Result<()> {
        match unsafe { SetWindowPlacement(self.clone(), &placement).as_bool() } {
            true => Ok(()),
            false => Err(Error::from_win32()),
        }
    }

    fn is_top_level(&self) -> bool {
        unsafe { GetAncestor(self.clone(), GA_ROOT).0 == self.0 }
    }

    fn owner(&self) -> Result<OwnerInfo> {
        let mut pid = 0u32;
        let tid = unsafe { GetWindowThreadProcessId(self.clone(), Some(&mut pid)) };

        if tid == 0 {
            Err(Error::from_win32())?;
        }

        Ok(OwnerInfo {
            process_id: pid,
            thread_id: tid,
        })
    }

    fn is_visible(&self) -> bool {
        unsafe { IsWindowVisible(self.clone()) }.as_bool()
    }
}

pub fn enum_windows<F: FnMut(HWND) -> bool>(mut cb: F) -> Result<()> {
    extern "system" fn enum_sys<F: FnMut(HWND) -> bool>(wnd: HWND, param: LPARAM) -> BOOL {
        let cb = unsafe { &mut *(param.0 as *mut F) };
        (cb)(wnd).into()
    }

    let ret = unsafe { EnumWindows(Some(enum_sys::<F>), LPARAM(&mut cb as *mut _ as isize)) };

    match ret.as_bool() {
        true => Ok(()),
        false => Err(Error::from_win32()),
    }
}

pub fn windows() -> Result<Vec<HWND>> {
    let mut vec = Vec::new();
    enum_windows(|wnd| {
        vec.push(wnd);

        true
    })?;

    Ok(vec)
}
