use windows::Win32::{
    Foundation::{HMODULE, HWND},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        WindowsAndMessaging::{WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS},
    },
};

use std::{cell::RefCell, collections::HashMap, rc::Rc};

type CallbackFn = dyn Fn(u32, HWND);

// Note that per MSDN, events are dispatched to the same thread that registered them.
thread_local! {
    static EVENT_TABLE: RefCell<HashMap<isize, EventHook>> = RefCell::new(HashMap::new());
}

fn current_module() -> HMODULE {
    unsafe { GetModuleHandleW(None) }.expect("failed to query current module")
}

pub struct EventHandle(HWINEVENTHOOK);

pub struct EventHook {
    cb: Rc<CallbackFn>,
}

#[allow(dead_code)]
impl EventHook {
    pub fn register_ranges(
        ranges: &[(u32, u32)],
        cb: impl Fn(u32, HWND) + 'static,
    ) -> Vec<EventHandle> {
        let ranges = ranges
            .into_iter()
            .map(|(min, max)| {
                EventHandle(unsafe {
                    SetWinEventHook(
                        *min,
                        *max,
                        current_module(),
                        Some(event_cb),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                    )
                })
            })
            .collect::<Vec<_>>();

        let cb = Rc::new(cb);
        EVENT_TABLE.with(|tab| {
            for hnd in &ranges {
                tab.borrow_mut()
                    .insert(hnd.0 .0, EventHook { cb: cb.clone() });
            }
        });

        ranges
    }

    pub fn register(evt_min: u32, evt_max: u32, cb: impl Fn(u32, HWND) + 'static) -> EventHandle {
        let hnd = EventHandle(unsafe {
            SetWinEventHook(
                evt_min,
                evt_max,
                current_module(),
                Some(event_cb),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        });

        // N.B: Despite us registering the event hook internally _after_ registering the callback with
        // the system, there is no race condition here because the callback is not invoked asynchronously.
        EVENT_TABLE.with(|tab| {
            tab.borrow_mut()
                .insert(hnd.0 .0, EventHook { cb: Rc::new(cb) });
        });

        hnd
    }

    pub fn unregister(handle: EventHandle) {
        unsafe { UnhookWinEvent(handle.0) };
    }
}

extern "system" fn event_cb(
    hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _idobject: i32,
    _idchild: i32,
    _ideventthread: u32,
    _dwmseventtime: u32,
) {
    EVENT_TABLE.with(|tab| {
        if let Some(hook) = tab.borrow().get(&hook.0) {
            (hook.cb)(event, hwnd);
        }
    });
}
