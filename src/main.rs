#![windows_subsystem = "windows"]

use std::{cell::RefCell, rc::Rc};

use anyhow::Context;
use log::{error, info, warn};
use mutex::GlobalMutex;
use nwd::NwgUi;
use nwg::{NativeUi, TrayNotificationFlags};
use rusqlite::{named_params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use widestring::widecstr;
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM},
        System::Threading::{GetExitCodeProcess, WaitForSingleObject, PROCESS_QUERY_INFORMATION},
        UI::{
            Shell::ShellExecuteExW,
            WindowsAndMessaging::{
                EVENT_OBJECT_NAMECHANGE, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MOVESIZESTART,
                SHOW_WINDOW_CMD, SW_MAXIMIZE, SW_SHOWNORMAL, WINDOWPLACEMENT, WM_DISPLAYCHANGE,
                WM_WTSSESSION_CHANGE, WPF_ASYNCWINDOWPLACEMENT,
            },
        },
    },
};

mod hook;
mod monitor;
mod mutex;
mod process;
mod window;

use hook::EventHook;
use monitor::HMonitorExt;
use window::HwndExt;
use winreg::enums::HKEY_CURRENT_USER;

use crate::process::ProcessExt;

const HKCU: winreg::RegKey = winreg::RegKey::predef(HKEY_CURRENT_USER);
const STARTUP_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run";
const STARTUP_NAME: &str = "PersistentWindows";

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
struct Topology {
    monitors: Vec<Rect>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct WindowDisplay {
    show: u32,
    min: Point,
    max: Point,
    rect: Rect,
}

impl From<WINDOWPLACEMENT> for WindowDisplay {
    fn from(wp: WINDOWPLACEMENT) -> Self {
        Self {
            show: wp.showCmd.0,
            min: wp.ptMinPosition.into(),
            max: wp.ptMaxPosition.into(),
            rect: wp.rcNormalPosition.into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl From<windows::Win32::Foundation::POINT> for Point {
    fn from(r: windows::Win32::Foundation::POINT) -> Self {
        Self { x: r.x, y: r.y }
    }
}

impl Into<windows::Win32::Foundation::POINT> for Point {
    fn into(self) -> windows::Win32::Foundation::POINT {
        windows::Win32::Foundation::POINT {
            x: self.x,
            y: self.y,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> u32 {
        (self.right - self.left).abs() as u32
    }

    pub fn height(&self) -> u32 {
        (self.bottom - self.top).abs() as u32
    }
}

impl From<windows::Win32::Foundation::RECT> for Rect {
    fn from(r: windows::Win32::Foundation::RECT) -> Self {
        Self {
            top: r.top,
            bottom: r.bottom,
            right: r.right,
            left: r.left,
        }
    }
}

impl Into<windows::Win32::Foundation::RECT> for Rect {
    fn into(self) -> windows::Win32::Foundation::RECT {
        windows::Win32::Foundation::RECT {
            top: self.top,
            bottom: self.bottom,
            right: self.right,
            left: self.left,
        }
    }
}

#[derive(Default)]
pub struct AppData {
    /// The current display topology index
    active_topology: Option<usize>,
}

#[derive(NwgUi)]
pub struct App {
    #[nwg_control(flags: "DISABLED")]
    #[nwg_events( OnInit: [App::on_init] )]
    window: nwg::Window,

    #[nwg_resource]
    embed: nwg::EmbedResource,

    #[nwg_resource(source_embed: Some(&data.embed), source_embed_str: Some("MAINICON"))]
    icon: nwg::Icon,

    #[nwg_control(icon: Some(&data.icon), tip: Some("Persistent Windows"))]
    #[nwg_events( MousePressLeftUp: [App::on_tray_click], OnContextMenu: [App::on_tray_click] )]
    tray: nwg::TrayNotification,

    #[nwg_control(parent: window, popup: true)]
    tray_menu: nwg::Menu,

    #[nwg_control(parent: tray_menu, text: "About")]
    #[nwg_events(OnMenuItemSelected: [App::on_about])]
    tray_menu_about: nwg::MenuItem,

    #[nwg_control(parent: tray_menu)]
    tray_menu_sep: nwg::MenuSeparator,

    #[nwg_control(parent: tray_menu, text: "Autorun", check: false)]
    #[nwg_events(OnMenuItemSelected: [App::on_autorun_toggle])]
    tray_menu_autorun: nwg::MenuItem,

    #[nwg_control(parent: tray_menu, text: "Exit")]
    #[nwg_events(OnMenuItemSelected: [App::on_exit])]
    tray_menu_exit: nwg::MenuItem,

    data: RefCell<AppData>,
    db: rusqlite::Connection,
}

impl App {
    fn new(conn: rusqlite::Connection) -> Self {
        Self {
            window: Default::default(),
            embed: Default::default(),
            icon: Default::default(),
            tray: Default::default(),
            tray_menu: Default::default(),
            tray_menu_about: Default::default(),
            tray_menu_sep: Default::default(),
            tray_menu_autorun: Default::default(),
            tray_menu_exit: Default::default(),
            data: RefCell::new(Default::default()),
            db: conn,
        }
    }

    fn has_autostart() -> std::io::Result<bool> {
        // Determine if we are already set to automatically start.
        let key = HKCU.open_subkey(STARTUP_KEY)?;
        if let Ok(_val) = key.get_value::<String, &str>("PersistentWindows") {
            Ok::<bool, std::io::Error>(true)
        } else {
            Ok::<bool, std::io::Error>(false)
        }
    }

    fn on_init(&self) {
        if let Ok(r) = Self::has_autostart() {
            self.tray_menu_autorun.set_checked(r);
        }
    }

    fn on_tray_click(&self) {
        let (x, y) = nwg::GlobalCursor::position();
        self.tray_menu.popup(x, y);
    }

    fn on_autorun_toggle(&self) {
        match runas_admin("autorun") {
            Ok(0) => {
                self.tray_menu_autorun
                    .set_checked(!self.tray_menu_autorun.checked());
            }
            Ok(_) => {}
            Err(e) => {
                nwg::modal_error_message(&self.window, "Error", &format!("{e:?}"));
                return;
            }
        };
    }

    fn on_about(&self) {
        self.tray.show(
            &format!(
                "Persistent Windows {}\n{}",
                env!("VERGEN_BUILD_SEMVER"),
                env!("VERGEN_GIT_SHA_SHORT")
            ),
            Some("About"),
            Some(TrayNotificationFlags::LARGE_ICON),
            Some(&self.icon),
        );
    }

    fn on_exit(&self) {
        nwg::stop_thread_dispatch();
    }

    fn find_window(
        &self,
        topology: usize,
        path: &str,
        class: &str,
        title: &str,
    ) -> Option<WindowDisplay> {
        if let Some(disp) = self
            .db
            .query_row(
                "SELECT disp FROM appwindow WHERE topology=:topology AND class=:class AND path=:path AND title=:title",
                named_params! { ":topology": topology, ":class": class, ":path": path, ":title": title },
                |r| r.get::<usize, Vec<u8>>(0),
            )
            .optional()
            .unwrap()
        {
            bson::from_reader(&*disp).unwrap()
        } else {
            None
        }
    }

    fn restore_windows(&self) -> anyhow::Result<()> {
        let handles = window::windows().context("failed to query windows")?;

        for hwnd in handles {
            // Silently ignore any errors for individual windows.
            let _ = self.restore_window(hwnd);
        }

        Ok(())
    }

    fn capture_windows(&self) -> anyhow::Result<()> {
        let handles = window::windows().context("failed to query windows")?;

        info!("capturing {} handles", handles.len());
        for hwnd in handles {
            // Silently ignore any errors for individual windows.
            match self
                .capture_window(hwnd)
                .context("failed to capture window")
            {
                Ok(_) => {
                    /*
                    if let Ok(title) = hwnd.title() {
                        info!("captured {}", title)
                    }
                    */
                }
                Err(e) => warn!("{e:?}"),
            }
        }

        Ok(())
    }

    fn restore_window(&self, hwnd: HWND) -> anyhow::Result<()> {
        let topology = self
            .data
            .borrow()
            .active_topology
            .expect("no active topology");

        if hwnd.is_visible() {
            let class_name = hwnd.class_name().context("failed to query class name")?;
            let title = hwnd.title().context("failed to query title")?;
            let placement = hwnd.placement().context("failed to query placement")?;

            let owner = hwnd.owner().context("failed to query window owner")?;
            let proc = process::open(PROCESS_QUERY_INFORMATION.0, owner.process_id)
                .context("failed to open process")?;

            let exe = proc
                .full_image_name()
                .context("failed to query process exe name")?;

            if let Some(restore_placement) = self.find_window(topology, &exe, &class_name, &title) {
                let wnd_placement = WINDOWPLACEMENT {
                    length: core::mem::size_of::<WINDOWPLACEMENT>() as u32,
                    flags: WPF_ASYNCWINDOWPLACEMENT,
                    showCmd: SHOW_WINDOW_CMD(restore_placement.show),
                    ptMinPosition: restore_placement.min.into(),
                    ptMaxPosition: restore_placement.max.into(),
                    rcNormalPosition: restore_placement.rect.into(),
                };

                match SHOW_WINDOW_CMD(restore_placement.show) {
                    SW_MAXIMIZE => {
                        // For some reason, maximized windows ignore SetWindowPlacement calls,
                        // so we have to set the window to normal placement first, and then maximize
                        // it afterwards.
                        let mut wnd_placement = wnd_placement.clone();
                        wnd_placement.showCmd = SW_SHOWNORMAL;
                        hwnd.set_placement(wnd_placement)
                            .context("failed to restore maximized window placement")?;
                    }
                    _ => info!(
                        "restoring {exe} - {class_name} from {:?} to {:?}",
                        placement.rcNormalPosition, wnd_placement.rcNormalPosition
                    ),
                };

                hwnd.set_placement(wnd_placement)
                    .context("failed to restore window placement")?;
            }
        }

        Ok(())
    }

    fn capture_window(&self, hwnd: HWND) -> anyhow::Result<()> {
        let topology = self
            .data
            .borrow()
            .active_topology
            .expect("no active topology");

        if hwnd.is_visible() && hwnd.is_top_level() {
            let class_name = hwnd.class_name().context("failed to query class name")?;
            let title = hwnd.title().context("failed to query title")?;
            let placement = hwnd.placement().context("failed to query placement")?;

            let owner = hwnd.owner().context("failed to query window owner")?;
            let proc = process::open(PROCESS_QUERY_INFORMATION.0, owner.process_id)
                .context("failed to open process")?;

            let exe = proc
                .full_image_name()
                .context("failed to query process exe name")?;

            /*
            println!(
                "{exe}: {class_name} {title} {:?}",
                placement.rcNormalPosition
            );
            */

            let mut rect = Vec::new();
            bson::to_document(&WindowDisplay::from(placement))
                .unwrap()
                .to_writer(&mut rect)
                .unwrap();

            self.db
                .execute(
                    "REPLACE INTO appwindow (path, topology, class, title, disp) VALUES (:path, :topology, :class, :title, :disp)",
                    named_params! { ":path": exe, ":topology": topology, ":class": &class_name, ":title": title, ":disp": rect },
                )
                .context("failed to query database")?;
        }

        Ok(())
    }

    fn capture_topology(&self) -> anyhow::Result<usize> {
        let monitors = monitor::monitors(None).context("failed to query display topology")?;

        let rects = monitors
            .into_iter()
            .map(|(m, _)| Ok(m.info()?.rect))
            .collect::<Result<Vec<_>, windows::core::Error>>()
            .context("failed to query monitor info")?;

        let mut topology = Vec::new();
        bson::to_document(&Topology { monitors: rects })
            .unwrap()
            .to_writer(&mut topology)
            .unwrap();

        // Register the new topology if it is not already in the database.
        self.db
            .execute(
                "INSERT OR IGNORE INTO topology (data) VALUES (:topology)",
                named_params! { ":topology": topology },
            )
            .context("failed to query database")?;

        // Retrieve the row ID and save it.
        let row_id = self
            .db
            .query_row(
                "SELECT rowid FROM topology WHERE data=:topology",
                named_params! { ":topology": topology },
                |row| row.get::<usize, usize>(0),
            )
            .context("failed to query row id")?;

        Ok(row_id)
    }

    /// This is called when a window event happens in the system
    fn on_wnd_event(&self, hwnd: HWND, _event: u32) {
        // Interesting system events:
        // - EVENT_SYSTEM_FOREGROUND (OS window foreground/background)
        // - EVENT_OBJECT_LOCATIONCHANGE
        // - EVENT_OBJECT_NAMECHANGE
        // - EVENT_OBJECT_DESTROY
        // - EVENT_SYSTEM_MOVESIZESTART
        // - EVENT_SYSTEM_MOVESIZEEND
        // - EVENT_SYSTEM_MINIMIZESTART
        // - EVENT_SYSTEM_MINIMIZEEND
        let _ = self.capture_window(hwnd);
    }

    fn on_raw_event(
        &self,
        _hwnd: HWND,
        msg: u32,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Option<LRESULT> {
        // Interesting events:
        // - WM_WTSSESSION_CHANGE (remote/console)
        // - WM_DISPLAYCHANGE (resolution change)
        match msg {
            WM_DISPLAYCHANGE => {
                // TODO: Query display topology and resolution, and use it as a key for looking up window layout.
                // TODO: Enumerate all windows in the active desktop, restore positioning if differs
                let topo_id = match self
                    .capture_topology()
                    .context("failed to capture topology")
                {
                    Ok(id) => id,
                    Err(e) => {
                        error!("{e}");
                        return None;
                    }
                };

                self.data.borrow_mut().active_topology = Some(topo_id);

                info!("display change: {topo_id}");
                self.restore_windows().unwrap();
            }
            WM_WTSSESSION_CHANGE => {}
            _ => {}
        }

        None
    }
}

fn runas_admin(params: &str) -> std::result::Result<i32, windows::core::Error> {
    let exe =
        widestring::WideCString::from_os_str(std::env::current_exe().unwrap().as_os_str()).unwrap();
    let params = widestring::WideCString::from_str(params).unwrap();

    let mut info = windows::Win32::UI::Shell::SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<windows::Win32::UI::Shell::SHELLEXECUTEINFOW>() as u32,
        fMask: windows::Win32::UI::Shell::SEE_MASK_NOCLOSEPROCESS,
        hwnd: HWND(0),
        lpVerb: PCWSTR(widecstr!("runas").as_ptr()),
        lpFile: PCWSTR(exe.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0 as i32,

        ..Default::default()
    };

    match unsafe { ShellExecuteExW(&mut info) }.as_bool() {
        true => {}
        false => Err(windows::core::Error::from_win32())?,
    }

    unsafe { WaitForSingleObject(info.hProcess, !0) };

    let mut code = 0u32;
    match unsafe { GetExitCodeProcess(info.hProcess, &mut code) }.as_bool() {
        true => {}
        false => Err(windows::core::Error::from_win32())?,
    }

    Ok(code as i32)
}

fn toggle_autorun() -> anyhow::Result<()> {
    let cur_state = App::has_autostart().context("could not determine auto-start state")?;

    let key = HKCU
        .open_subkey_with_flags(
            STARTUP_KEY,
            winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
        )
        .context("could not open registry key")?;
    match cur_state {
        true => {
            // Disable autorun.
            key.delete_value(STARTUP_NAME)
                .context("failed to delete startup value")?;
        }
        false => {
            // Enable autorun.
            key.set_value(
                STARTUP_NAME,
                &std::env::current_exe()
                    .context("failed to query exe name")?
                    .as_os_str(),
            )
            .context("failed to set startup value")?;
        }
    };

    Ok(())
}

fn run() -> anyhow::Result<()> {
    // Attempt to create a global mutex for this process.
    // If it fails, that means we have another instance running.
    let _mutex = match GlobalMutex::create("Global\\{D1905271-98BC-4888-BC9D-B05810AA21CB}", true) {
        Ok(g) => g,
        Err(e) => match e.code() {
            e if e == ERROR_ALREADY_EXISTS.to_hresult() => anyhow::bail!("app is already running"),
            _ => Err(e).context("failed to create singleton mutex")?,
        },
    };

    let db = Connection::open_in_memory().context("Failed to open DB")?;
    db.execute_batch(
        "CREATE TABLE appwindow (
                path        TEXT NOT NULL,
                topology    INTEGER NOT NULL,
                class       STRING NOT NULL,
                title       TEXT NOT NULL,
                disp        BLOB NOT NULL,
                PRIMARY KEY (path, topology, class, title),
                FOREIGN KEY (topology) REFERENCES topology(id)
            );
            CREATE TABLE topology (
                id          INTEGER PRIMARY KEY,
                data        BLOB UNIQUE NOT NULL
            );",
    )
    .unwrap();

    let app = Rc::new(App::build_ui(App::new(db)).context("Failed to build UI")?);

    // This notification is annoying, so only show it on release builds.
    if false {
        app.tray.show(
            "Persistent Windows",
            Some("Started tracking windows"),
            Some(nwg::TrayNotificationFlags::USER_ICON | nwg::TrayNotificationFlags::LARGE_ICON),
            Some(&app.icon),
        );
    }

    let topo_id = app
        .capture_topology()
        .context("failed to capture initial topology")?;
    app.data.borrow_mut().active_topology = Some(topo_id);

    app.capture_windows()
        .context("failed to capture initial window set")?;

    // Handle a few raw events as well.
    let appref = Rc::downgrade(&app);
    let raw_hook = nwg::bind_raw_event_handler(
        &app.window.handle,
        0x13370,
        move |hwnd, msg, wparam, lparam| {
            if let Some(app) = appref.upgrade() {
                app.on_raw_event(HWND(hwnd as isize), msg, WPARAM(wparam), LPARAM(lparam))
                    .map(|r| r.0)
            } else {
                None
            }
        },
    )
    .context("could not bind raw handler")?;

    let appref = Rc::downgrade(&app);
    let evt_hooks = EventHook::register_ranges(
        &[
            (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MINIMIZEEND),
            (EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE),
        ],
        move |evt, wnd| {
            if let Some(app) = appref.upgrade() {
                app.on_wnd_event(wnd, evt);
            }
        },
    );

    nwg::dispatch_thread_events();

    for hook in evt_hooks {
        EventHook::unregister(hook);
    }

    nwg::unbind_raw_event_handler(&raw_hook).unwrap();

    Ok(())
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    // Check and see if we were invoked to run a utility command.
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() > 1 {
        let res = match args[1].as_str() {
            "autorun" => toggle_autorun(),
            _ => anyhow::bail!("unknown command"),
        };

        return match res {
            Ok(_) => Ok(()),
            Err(e) => {
                nwg::error_message("Error", &format!("{e:?}"));
                Err(e)
            }
        };
    }

    nwg::init().context("Failed to init NWG")?;
    nwg::Font::set_global_family("Segoe UI").context("Failed to set default font")?;

    // Display an error dialog if the run function fails (instead of logging to console, which is unavailable
    // in the Windows subsystem).
    match run() {
        Ok(_) => Ok(()),
        Err(e) => nwg::fatal_message("Error", &format!("{e:?}")),
    }
}
