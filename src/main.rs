#![windows_subsystem = "windows"]

use std::{cell::RefCell, rc::Rc};

use anyhow::Context;
use log::{error, info, warn};
use mutex::GlobalMutex;
use nwd::NwgUi;
use nwg::{NativeUi, TrayNotificationFlags};
use rusqlite::{named_params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use windows::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM},
    System::Threading::PROCESS_QUERY_INFORMATION,
    UI::WindowsAndMessaging::{
        EVENT_OBJECT_NAMECHANGE, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MOVESIZESTART,
        SHOW_WINDOW_CMD, SW_MAXIMIZE, SW_SHOWNORMAL, WINDOWPLACEMENT, WM_DISPLAYCHANGE,
        WM_WTSSESSION_CHANGE, WPF_ASYNCWINDOWPLACEMENT,
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

use crate::process::ProcessExt;

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
            tray_menu_exit: Default::default(),
            data: RefCell::new(Default::default()),
            db: conn,
        }
    }

    fn on_tray_click(&self) {
        let (x, y) = nwg::GlobalCursor::position();
        self.tray_menu.popup(x, y);
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

fn main() -> anyhow::Result<()> {
    env_logger::init();

    // Attempt to create a global mutex for this process.
    // If it fails, that means we have another instance running.
    let _mutex = match GlobalMutex::create("Global\\{D1905271-98BC-4888-BC9D-B05810AA21CB}", true) {
        Ok(g) => g,
        Err(e) => match e.code() {
            e if e == ERROR_ALREADY_EXISTS.to_hresult() => anyhow::bail!("app is already running"),
            _ => Err(e).context("failed to create singleton mutex")?,
        },
    };

    nwg::init().context("Failed to init NWG")?;
    nwg::Font::set_global_family("Segoe UI").context("Failed to set default font")?;

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
