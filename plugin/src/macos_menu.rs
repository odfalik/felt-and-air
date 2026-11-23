//! Minimal native macOS menu bar for the standalone app so Cmd+Q, Cmd+W, Cmd+H, Cmd+M
//! and standard macOS app-menu actions work without adding extra crate dependencies.

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_char, c_void, CString};

    type Id = *mut c_void;
    type Sel = *mut c_void;
    type Cls = *mut c_void;

    const NS_COMMAND_KEY_MASK: usize = 1 << 20;
    const NS_OPTION_KEY_MASK: usize = 1 << 19;

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> Cls;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_msgSend();
    }

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}

    unsafe fn cls(name: &str) -> Cls {
        let c = CString::new(name).unwrap();
        objc_getClass(c.as_ptr())
    }

    unsafe fn sel(name: &str) -> Sel {
        let c = CString::new(name).unwrap();
        sel_registerName(c.as_ptr())
    }

    unsafe fn msg0(obj: Id, s: Sel) -> Id {
        let f: extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as *const ());
        f(obj, s)
    }

    unsafe fn msg1_id(obj: Id, s: Sel, a: Id) -> Id {
        let f: extern "C" fn(Id, Sel, Id) -> Id = std::mem::transmute(objc_msgSend as *const ());
        f(obj, s, a)
    }

    unsafe fn msg1_usize(obj: Id, s: Sel, a: usize) {
        let f: extern "C" fn(Id, Sel, usize) = std::mem::transmute(objc_msgSend as *const ());
        f(obj, s, a);
    }

    unsafe fn nsstring(text: &str) -> Id {
        let c = CString::new(text).unwrap();
        let f: extern "C" fn(Cls, Sel, *const c_char) -> Id =
            std::mem::transmute(objc_msgSend as *const ());
        f(cls("NSString"), sel("stringWithUTF8String:"), c.as_ptr())
    }

    unsafe fn add_item(menu: Id, title: &str, action: Option<&str>, key: &str, mods: usize) {
        let item = msg0(cls("NSMenuItem"), sel("alloc"));
        let f: extern "C" fn(Id, Sel, Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const ());
        let action_sel = action.map(|a| sel(a)).unwrap_or(std::ptr::null_mut());
        let item = f(
            item,
            sel("initWithTitle:action:keyEquivalent:"),
            nsstring(title),
            action_sel,
            nsstring(key),
        );
        if mods != 0 {
            msg1_usize(item, sel("setKeyEquivalentModifierMask:"), mods);
        }
        msg1_id(menu, sel("addItem:"), item);
    }

    pub fn install() {
        unsafe {
            let pool = msg0(msg0(cls("NSAutoreleasePool"), sel("alloc")), sel("init"));
            let app = msg0(cls("NSApplication"), sel("sharedApplication"));
            if app.is_null() {
                msg0(pool, sel("drain"));
                return;
            }
            let main_menu = msg0(msg0(cls("NSMenu"), sel("alloc")), sel("init"));

            // Application menu ("Felt & Air")
            let app_item = msg0(msg0(cls("NSMenuItem"), sel("alloc")), sel("init"));
            msg1_id(main_menu, sel("addItem:"), app_item);
            let app_menu = msg1_id(
                msg0(cls("NSMenu"), sel("alloc")),
                sel("initWithTitle:"),
                nsstring("Felt & Air"),
            );
            add_item(app_menu, "Hide Felt & Air", Some("hide:"), "h", NS_COMMAND_KEY_MASK);
            add_item(
                app_menu,
                "Hide Others",
                Some("hideOtherApplications:"),
                "h",
                NS_COMMAND_KEY_MASK | NS_OPTION_KEY_MASK,
            );
            add_item(app_menu, "Show All", Some("unhideAllApplications:"), "", 0);
            let sep = msg0(cls("NSMenuItem"), sel("separatorItem"));
            msg1_id(app_menu, sel("addItem:"), sep);
            add_item(
                app_menu,
                "Quit Felt & Air",
                Some("terminate:"),
                "q",
                NS_COMMAND_KEY_MASK,
            );
            msg1_id(app_item, sel("setSubmenu:"), app_menu);

            // Window menu ("Window")
            let win_item = msg0(msg0(cls("NSMenuItem"), sel("alloc")), sel("init"));
            msg1_id(main_menu, sel("addItem:"), win_item);
            let win_menu = msg1_id(
                msg0(cls("NSMenu"), sel("alloc")),
                sel("initWithTitle:"),
                nsstring("Window"),
            );
            add_item(
                win_menu,
                "Minimize",
                Some("performMiniaturize:"),
                "m",
                NS_COMMAND_KEY_MASK,
            );
            add_item(
                win_menu,
                "Close",
                Some("performClose:"),
                "w",
                NS_COMMAND_KEY_MASK,
            );
            msg1_id(win_item, sel("setSubmenu:"), win_menu);

            msg1_id(app, sel("setMainMenu:"), main_menu);
            msg1_id(app, sel("setWindowsMenu:"), win_menu);
            msg0(pool, sel("drain"));
        }
    }
}

pub fn install() {
    #[cfg(target_os = "macos")]
    imp::install();
}
