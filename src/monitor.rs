use crate::Rect;

use serde::{Deserialize, Serialize};
use windows::{
    core::Error,
    Win32::{
        Foundation::{BOOL, LPARAM, RECT},
        Graphics::Gdi::{
            EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
        },
        UI::WindowsAndMessaging::MONITORINFOF_PRIMARY,
    },
};

type Result<R> = core::result::Result<R, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// Whether or not the monitor is the primary monitor
    pub primary: bool,
    /// A rectangle specifying the monitor's area in virtual screen coordinates
    pub rect: Rect,
    /// A rectangle specifying the monitor's work area
    pub work: Rect,
    /// The name of the monitor
    pub name: String,
}

pub trait HMonitorExt {
    fn info(&self) -> Result<MonitorInfo>;
}

impl HMonitorExt for HMONITOR {
    fn info(&self) -> Result<MonitorInfo> {
        let mut info: MONITORINFOEXW = Default::default();
        info.monitorInfo.cbSize = core::mem::size_of::<MONITORINFOEXW>() as u32;

        match unsafe {
            GetMonitorInfoW(self.clone(), &mut info as *mut _ as *mut MONITORINFO).as_bool()
        } {
            true => {
                let name = String::from_utf16_lossy(&info.szDevice);

                Ok(MonitorInfo {
                    primary: (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0,
                    rect: info.monitorInfo.rcMonitor.into(),
                    work: info.monitorInfo.rcWork.into(),
                    name,
                })
            }
            false => Err(Error::from_win32()),
        }
    }
}

pub fn enum_monitors<F: FnMut(HMONITOR, HDC, Option<&mut RECT>) -> bool>(
    rect: Option<RECT>,
    mut cb: F,
) -> Result<()> {
    extern "system" fn enum_sys<F: FnMut(HMONITOR, HDC, Option<&mut RECT>) -> bool>(
        mon: HMONITOR,
        dc: HDC,
        rect: *mut RECT,
        param: LPARAM,
    ) -> BOOL {
        let cb = unsafe { &mut *(param.0 as *mut F) };

        (cb)(mon, dc, unsafe { rect.as_mut() }).into()
    }

    let ret = unsafe {
        EnumDisplayMonitors(
            None,
            rect.as_ref()
                .map(|c| c as *const RECT)
                .unwrap_or(core::ptr::null()),
            Some(enum_sys::<F>),
            LPARAM(&mut cb as *mut _ as isize),
        )
    };

    match ret.as_bool() {
        true => Ok(()),
        false => Err(Error::from_win32()),
    }
}

pub fn monitors(rect: Option<RECT>) -> Result<Vec<(HMONITOR, HDC)>> {
    let mut vec = Vec::new();
    enum_monitors(rect, |hmon, hdc, _rect| {
        vec.push((hmon, hdc));

        true
    })?;

    Ok(vec)
}
