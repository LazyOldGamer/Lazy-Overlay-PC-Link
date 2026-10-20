//! The icon by the clock (Windows only): kept while the window is hidden,
//! shown for notifications, marked with the connection's state. A click opens
//! the window; its menu opens or quits. A second copy wakes the first through it.

use std::sync::Arc;

/// What was asked of the window: by the icon, or by PC Link opened again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    Open,
    Quit,
}

/// Told each click, from the icon's own thread.
pub type OnClick = Arc<dyn Fn(Click) + Send + Sync>;

/// The icon's mark: none while connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Fine,
    /// Connecting, or trying again: yellow.
    Waiting,
    /// Needs the streamer: red.
    Problem,
}

/// Windows only.
pub const fn supported() -> bool {
    cfg!(windows)
}

/// The window's: told the icon's clicks, and when PC Link is opened again.
pub fn listen(on_click: OnClick) {
    imp::listen(on_click)
}

/// Keeps the icon by the clock, or lets it go once no notification needs it.
pub fn keep(on: bool) {
    imp::keep(on)
}

/// How the connection stands, in the icon's mark and tip.
pub fn status(mark: Mark, words: &str) {
    imp::status(mark, words)
}

/// Takes the icon away now, as PC Link ends, so none is left behind.
pub fn end() {
    imp::end()
}

/// A second copy: asks the running one's window to come forward; whether it
/// will.
pub fn wake_other() -> bool {
    imp::wake_other()
}

#[cfg(not(windows))]
mod imp {
    pub fn listen(_: super::OnClick) {}
    pub fn keep(_: bool) {}
    pub fn status(_: super::Mark, _: &str) {}
    pub fn end() {}
    pub fn wake_other() -> bool {
        false
    }
}

/// A round mark of `color`, ringed in the window's background, over the
/// lower right of BGRA pixels (straight alpha), smoothed at its edge.
#[cfg_attr(not(windows), allow(dead_code))]
fn mark(pixels: &mut [u8], size: usize, color: [u8; 3]) {
    let r = size as f32 * 0.24;
    let c = size as f32 * 0.72;
    for (n, px) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = ((n % size) as f32 + 0.5, (n / size) as f32 + 0.5);
        let d = ((x - c).powi(2) + (y - c).powi(2)).sqrt();
        for (paint, radius) in [([0x0e, 0x11, 0x1b], r + 1.0), (color, r)] {
            let a = (radius - d + 0.5).clamp(0.0, 1.0);
            if a == 0.0 {
                continue;
            }
            let under = px[3] as f32 / 255.0;
            let out = a + under * (1.0 - a);
            for (i, channel) in [paint[2], paint[1], paint[0]].into_iter().enumerate() {
                px[i] = ((channel as f32 * a + px[i] as f32 * under * (1.0 - a)) / out).round() as u8;
            }
            px[3] = (out * 255.0).round() as u8;
        }
    }
}

#[cfg(windows)]
pub(crate) use imp::note;

// One thread owns a hidden window and the icon: added while kept or while a
// notification shows, removed once neither needs it.
#[cfg(windows)]
mod imp {
    use super::{Click, Mark, OnClick, mark};
    use crate::os::wide;
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    type Hwnd = isize;
    type Handle = *mut c_void;
    type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

    const NIM_ADD: u32 = 0;
    const NIM_MODIFY: u32 = 1;
    const NIM_DELETE: u32 = 2;
    const NIF_MESSAGE: u32 = 0x1;
    const NIF_ICON: u32 = 0x2;
    const NIF_TIP: u32 = 0x4;
    const NIF_INFO: u32 = 0x10;
    const NIIF_INFO: u32 = 0x1;
    const NIIF_ERROR: u32 = 0x3;
    const PM_REMOVE: u32 = 0x1;
    const IDI_APPLICATION: usize = 32512;
    /// The program's own icon (build.rs, winresource).
    const OWN_ICON: usize = 1;
    const IMAGE_ICON: u32 = 1;
    const SM_CXSMICON: i32 = 49;
    const DIB_RGB_COLORS: u32 = 0;
    /// What the icon sends this window: the mouse message in the low word.
    const WM_ICON: u32 = 0x8000 + 1;
    /// From a second copy: come forward. Answered 1 when the window listens.
    const WM_WAKE: u32 = 0x8000 + 2;
    const SMTO_ABORTIFHUNG: u32 = 0x2;
    const WM_NULL: u32 = 0;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_RBUTTONUP: u32 = 0x0205;
    const MF_STRING: u32 = 0;
    const MF_SEPARATOR: u32 = 0x800;
    const TPM_RIGHTBUTTON: u32 = 0x2;
    const TPM_NONOTIFY: u32 = 0x80;
    const TPM_RETURNCMD: u32 = 0x100;
    const OPEN: usize = 1;
    const QUIT: usize = 2;
    const CLASS: &str = "LazyOverlayPCLinkIcon";
    const TIP: &str = "Lazy Overlay PC Link";
    /// The marks' colors, as the window's (gui.rs): yellow, red.
    const YELLOW: [u8; 3] = [0xff, 0xb6, 0x38];
    const RED: [u8; 3] = [0xff, 0x4d, 0x4d];

    #[repr(C)]
    struct NotifyIconData {
        size: u32,
        hwnd: Hwnd,
        id: u32,
        flags: u32,
        callback_message: u32,
        icon: Handle,
        tip: [u16; 128],
        state: u32,
        state_mask: u32,
        info: [u16; 256],
        timeout_or_version: u32,
        info_title: [u16; 64],
        info_flags: u32,
        guid: [u8; 16],
        balloon_icon: Handle,
    }

    #[repr(C)]
    struct Msg {
        hwnd: Hwnd,
        message: u32,
        w: usize,
        l: isize,
        time: u32,
        x: i32,
        y: i32,
        private: u32,
    }

    #[repr(C)]
    struct WndClass {
        style: u32,
        proc: WndProc,
        class_extra: i32,
        window_extra: i32,
        instance: Handle,
        icon: Handle,
        cursor: Handle,
        background: Handle,
        menu_name: *const u16,
        class_name: *const u16,
    }

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct IconInfo {
        is_icon: i32,
        x_hot: u32,
        y_hot: u32,
        mask: Handle,
        color: Handle,
    }

    #[repr(C)]
    struct BitmapInfo {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bit_count: u16,
        compression: u32,
        size_image: u32,
        x_ppm: i32,
        y_ppm: i32,
        used: u32,
        important: u32,
        colors: [u32; 3],
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterClassW(class: *const WndClass) -> u16;
        fn RegisterWindowMessageW(name: *const u16) -> u32;
        fn CreateWindowExW(
            ex_style: u32,
            class: *const u16,
            name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            parent: Hwnd,
            menu: Handle,
            instance: Handle,
            param: Handle,
        ) -> Hwnd;
        fn DefWindowProcW(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize;
        fn LoadIconW(instance: Handle, name: *const u16) -> Handle;
        fn LoadImageW(instance: Handle, name: *const u16, kind: u32, w: i32, h: i32, flags: u32) -> Handle;
        fn GetSystemMetrics(index: i32) -> i32;
        fn GetIconInfo(icon: Handle, info: *mut IconInfo) -> i32;
        fn CreateIconIndirect(info: *const IconInfo) -> Handle;
        fn GetDC(hwnd: Hwnd) -> Handle;
        fn ReleaseDC(hwnd: Hwnd, dc: Handle) -> i32;
        fn PeekMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> i32;
        fn TranslateMessage(msg: *const Msg) -> i32;
        fn DispatchMessageW(msg: *const Msg) -> isize;
        fn PostMessageW(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> i32;
        fn SendMessageTimeoutW(hwnd: Hwnd, msg: u32, w: usize, l: isize, flags: u32, ms: u32, result: *mut usize) -> isize;
        fn FindWindowW(class: *const u16, name: *const u16) -> Hwnd;
        fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
        fn AllowSetForegroundWindow(pid: u32) -> i32;
        fn CreatePopupMenu() -> Handle;
        fn AppendMenuW(menu: Handle, flags: u32, id: usize, text: *const u16) -> i32;
        fn SetMenuDefaultItem(menu: Handle, item: u32, by_position: u32) -> i32;
        fn TrackPopupMenu(menu: Handle, flags: u32, x: i32, y: i32, reserved: i32, hwnd: Hwnd, rect: *const c_void) -> i32;
        fn DestroyMenu(menu: Handle) -> i32;
        fn GetCursorPos(point: *mut Point) -> i32;
        fn SetForegroundWindow(hwnd: Hwnd) -> i32;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn GetDIBits(dc: Handle, bitmap: Handle, start: u32, lines: u32, bits: *mut c_void, info: *mut BitmapInfo, usage: u32) -> i32;
        fn CreateDIBSection(
            dc: Handle,
            info: *const BitmapInfo,
            usage: u32,
            bits: *mut *mut c_void,
            section: Handle,
            offset: u32,
        ) -> Handle;
        fn CreateBitmap(w: i32, h: i32, planes: u32, bits: u32, data: *const c_void) -> Handle;
        fn DeleteObject(object: Handle) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> Handle;
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn Shell_NotifyIconW(message: u32, data: *mut NotifyIconData) -> i32;
    }

    // `s` into a fixed field, cut to fit (between characters, never inside
    // one) and always ended.
    fn fill(field: &mut [u16], s: &str) {
        let mut units: Vec<u16> = Vec::new();
        for c in s.chars() {
            let mut two = [0u16; 2];
            let c = c.encode_utf16(&mut two);
            if units.len() + c.len() > field.len() - 1 {
                break;
            }
            units.extend_from_slice(c);
        }
        field[..units.len()].copy_from_slice(&units);
        field[units.len()] = 0;
    }

    enum Ask {
        Note { title: String, text: String, error: bool },
        Listen(OnClick),
        Keep(bool),
        Status(Mark, String),
        End(Sender<()>),
    }

    static SEND: OnceLock<Mutex<Sender<Ask>>> = OnceLock::new();

    fn ask(a: Ask) -> Result<(), String> {
        let send = SEND.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Ask>();
            std::thread::spawn(move || run(rx));
            Mutex::new(tx)
        });
        send.lock().map_err(|_| "notifications stopped".to_string())?.send(a).map_err(|_| "notifications stopped".to_string())
    }

    pub fn note(title: &str, text: &str, error: bool) -> Result<(), String> {
        ask(Ask::Note { title: title.to_string(), text: text.to_string(), error })
    }

    pub fn listen(on_click: OnClick) {
        let _ = ask(Ask::Listen(on_click));
    }

    pub fn keep(on: bool) {
        let _ = ask(Ask::Keep(on));
    }

    pub fn status(mark: Mark, words: &str) {
        let _ = ask(Ask::Status(mark, words.to_string()));
    }

    pub fn end() {
        let (tx, rx) = mpsc::channel();
        if SEND.get().is_some() && ask(Ask::End(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(1));
        }
    }

    pub fn wake_other() -> bool {
        let class = wide(CLASS);
        // SAFETY: a zero-terminated string; the window found is only sent
        // our own message, waited on at most 3 seconds, and allowed to come
        // forward (this copy was just opened, so it may let it).
        unsafe {
            let hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
            if hwnd == 0 {
                return false;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            AllowSetForegroundWindow(pid);
            let mut answer = 0usize;
            SendMessageTimeoutW(hwnd, WM_WAKE, 0, 0, SMTO_ABORTIFHUNG, 3000, &mut answer) != 0 && answer == 1
        }
    }

    /// The icon thread's own state, for the loop and the window procedure.
    struct Icon {
        data: NotifyIconData,
        /// The icon for each mark: Fine, Waiting, Problem.
        icons: [Handle; 3],
        added: bool,
        shown_at: Option<Instant>,
        kept: bool,
        mark: Mark,
        words: String,
        on_click: Option<OnClick>,
    }

    impl Icon {
        // Added, changed or removed as needed: its mark, and a tip saying how
        // the connection stands and whether a click opens the window.
        fn sync(&mut self) {
            let want = self.kept || self.shown_at.is_some();
            let mut tip = String::from(TIP);
            if !self.words.is_empty() {
                tip += &format!("\n{}", self.words);
            }
            if self.on_click.is_some() {
                tip += "\nClick to open";
            }
            fill(&mut self.data.tip, &tip);
            self.data.icon = self.icons[self.mark as usize];
            self.data.flags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            // SAFETY: data is this thread's, alive across each call.
            unsafe {
                match (want, self.added) {
                    (true, false) => self.added = Shell_NotifyIconW(NIM_ADD, &mut self.data) != 0,
                    (true, true) => _ = Shell_NotifyIconW(NIM_MODIFY, &mut self.data),
                    (false, true) => {
                        Shell_NotifyIconW(NIM_DELETE, &mut self.data);
                        self.added = false;
                    }
                    (false, false) => {}
                }
            }
        }
    }

    thread_local! {
        static ICON: RefCell<Option<Icon>> = const { RefCell::new(None) };
        /// Sent to every window when Explorer restarts: the icon is added again.
        static TASKBAR_CREATED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        /// Explorer restarted since the loop last looked.
        static READD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    // A left click on the icon opens the window, a right click its menu; a
    // second copy's wake opens it too.
    unsafe extern "system" fn proc(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize {
        // Never a second borrow: Windows can call this while the icon is being
        // changed (another copy's wake, sent as Shell_NotifyIconW waits), and
        // a borrow then would end PC Link. Busy, it is as if no one listened.
        let on_click = || ICON.with(|c| c.try_borrow().ok().and_then(|i| i.as_ref().and_then(|i| i.on_click.clone())));
        if msg == WM_ICON {
            if let Some(on_click) = on_click() {
                match (l & 0xffff) as u32 {
                    WM_LBUTTONUP => on_click(Click::Open),
                    WM_RBUTTONUP => {
                        if let Some(c) = menu(hwnd) {
                            on_click(c)
                        }
                    }
                    _ => {}
                }
            }
            return 0;
        }
        if msg == WM_WAKE {
            return match on_click() {
                Some(on_click) => {
                    on_click(Click::Open);
                    1
                }
                None => 0,
            };
        }
        // Added again by the loop, outside any borrow.
        if msg != 0 && msg == TASKBAR_CREATED.get() {
            READD.set(true);
            return 0;
        }
        // SAFETY: the arguments Windows gave, passed on.
        unsafe { DefWindowProcW(hwnd, msg, w, l) }
    }

    // The icon's menu at the pointer; the choice, if any.
    fn menu(hwnd: Hwnd) -> Option<Click> {
        let (open, quit) = (wide("Open PC Link"), wide("Quit PC Link"));
        // SAFETY: a menu made, shown and destroyed here; the strings outlive
        // it. The window comes forward first, so the menu closes on a click
        // elsewhere, and the posted message after lets it go.
        unsafe {
            let m = CreatePopupMenu();
            if m.is_null() {
                return None;
            }
            AppendMenuW(m, MF_STRING, OPEN, open.as_ptr());
            AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
            AppendMenuW(m, MF_STRING, QUIT, quit.as_ptr());
            SetMenuDefaultItem(m, OPEN as u32, 0);
            let mut at = Point { x: 0, y: 0 };
            GetCursorPos(&mut at);
            SetForegroundWindow(hwnd);
            let chosen = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, at.x, at.y, 0, hwnd, std::ptr::null());
            PostMessageW(hwnd, WM_NULL, 0, 0);
            DestroyMenu(m);
            match chosen as usize {
                OPEN => Some(Click::Open),
                QUIT => Some(Click::Quit),
                _ => None,
            }
        }
    }

    // The base icon with a mark, or the base icon when one cannot be made.
    fn marked(base: Handle, color: [u8; 3]) -> Handle {
        // SAFETY: the base's bitmaps are read into a buffer of their size and
        // deleted; the new bitmaps are copied by CreateIconIndirect, then
        // deleted. Every handle is checked before use.
        unsafe {
            let mut info: IconInfo = std::mem::zeroed();
            if GetIconInfo(base, &mut info) == 0 {
                return base;
            }
            let mut made = base;
            let dc = GetDC(0);
            let mut bmi: BitmapInfo = std::mem::zeroed();
            bmi.size = (std::mem::size_of::<BitmapInfo>() - 12) as u32;
            if !info.color.is_null()
                && !dc.is_null()
                && GetDIBits(dc, info.color, 0, 0, std::ptr::null_mut(), &mut bmi, DIB_RGB_COLORS) != 0
            {
                let size = bmi.width.max(1);
                (bmi.height, bmi.bit_count, bmi.compression, bmi.planes) = (-size, 32, 0, 1);
                let mut pixels = vec![0u8; (size * size * 4) as usize];
                if GetDIBits(dc, info.color, 0, size as u32, pixels.as_mut_ptr().cast(), &mut bmi, DIB_RGB_COLORS) == size {
                    mark(&mut pixels, size as usize, color);
                    let mut bits: *mut c_void = std::ptr::null_mut();
                    let color_bitmap = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
                    let zeros = vec![0u8; (size as usize).div_ceil(16) * 2 * size as usize];
                    let mask = CreateBitmap(size, size, 1, 1, zeros.as_ptr().cast());
                    if !color_bitmap.is_null() && !bits.is_null() && !mask.is_null() {
                        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
                        let new = IconInfo { is_icon: 1, x_hot: 0, y_hot: 0, mask, color: color_bitmap };
                        let icon = CreateIconIndirect(&new);
                        if !icon.is_null() {
                            made = icon;
                        }
                    }
                    for b in [color_bitmap, mask] {
                        if !b.is_null() {
                            DeleteObject(b);
                        }
                    }
                }
            }
            if !dc.is_null() {
                ReleaseDC(0, dc);
            }
            for b in [info.color, info.mask] {
                if !b.is_null() {
                    DeleteObject(b);
                }
            }
            made
        }
    }

    fn run(rx: mpsc::Receiver<Ask>) {
        let class = wide(CLASS);
        let name = wide(TIP);
        // SAFETY: this thread's class and hidden window, for good: a top-level
        // one, never shown, so the icon's menu can come forward. The strings
        // outlive the calls; a null module is the system's own icons.
        let (hwnd, icon) = unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let wc = WndClass {
                style: 0,
                proc,
                class_extra: 0,
                window_extra: 0,
                instance,
                icon: std::ptr::null_mut(),
                cursor: std::ptr::null_mut(),
                background: std::ptr::null_mut(),
                menu_name: std::ptr::null(),
                class_name: class.as_ptr(),
            };
            RegisterClassW(&wc);
            TASKBAR_CREATED.set(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()));
            let hwnd =
                CreateWindowExW(0, class.as_ptr(), name.as_ptr(), 0, 0, 0, 0, 0, 0, std::ptr::null_mut(), instance, std::ptr::null_mut());
            // The small size the taskbar's corner shows, so the mark is drawn at it.
            let small = GetSystemMetrics(SM_CXSMICON).max(16);
            let own = LoadImageW(instance, OWN_ICON as *const u16, IMAGE_ICON, small, small, 0);
            (hwnd, if own.is_null() { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION as *const u16) } else { own })
        };
        if hwnd == 0 {
            return;
        }
        let data = NotifyIconData {
            size: std::mem::size_of::<NotifyIconData>() as u32,
            hwnd,
            id: 1,
            flags: 0,
            callback_message: WM_ICON,
            icon,
            tip: [0; 128],
            state: 0,
            state_mask: 0,
            info: [0; 256],
            timeout_or_version: 0,
            info_title: [0; 64],
            info_flags: 0,
            guid: [0; 16],
            balloon_icon: std::ptr::null_mut(),
        };
        let icons = [icon, marked(icon, YELLOW), marked(icon, RED)];
        ICON.set(Some(Icon {
            data,
            icons,
            added: false,
            shown_at: None,
            kept: false,
            mark: Mark::Fine,
            words: String::new(),
            on_click: None,
        }));
        loop {
            let asked = rx.recv_timeout(Duration::from_millis(200));
            if matches!(asked, Err(RecvTimeoutError::Disconnected)) {
                break;
            }
            let ended = ICON.with_borrow_mut(|i| {
                let Some(i) = i else { return true };
                match asked {
                    Ok(Ask::Note { title, text, error }) => {
                        i.shown_at = Some(Instant::now());
                        i.sync();
                        i.data.flags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_INFO;
                        fill(&mut i.data.info_title, &title);
                        fill(&mut i.data.info, if text.is_empty() { " " } else { &text });
                        i.data.info_flags = if error { NIIF_ERROR } else { NIIF_INFO };
                        // SAFETY: data is this thread's, alive across the call.
                        unsafe { Shell_NotifyIconW(NIM_MODIFY, &mut i.data) };
                    }
                    Ok(Ask::Listen(on_click)) => {
                        i.on_click = Some(on_click);
                        i.sync();
                    }
                    Ok(Ask::Keep(on)) => {
                        i.kept = on;
                        i.sync();
                    }
                    Ok(Ask::Status(mark, words)) => {
                        (i.mark, i.words) = (mark, words);
                        i.sync();
                    }
                    Ok(Ask::End(done)) => {
                        (i.kept, i.shown_at) = (false, None);
                        i.sync();
                        let _ = done.send(());
                        return true;
                    }
                    Err(_) => {}
                }
                if READD.take() {
                    i.added = false;
                    i.sync();
                }
                if i.shown_at.is_some_and(|t| t.elapsed() >= Duration::from_secs(15)) {
                    i.shown_at = None;
                    i.sync();
                }
                false
            });
            if ended {
                break;
            }
            // SAFETY: msg is a local the call fills; only this thread's window.
            unsafe {
                let mut msg: Msg = std::mem::zeroed();
                while PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_mark_covers_the_lower_right_and_leaves_the_rest() {
        let size = 16;
        let mut pixels = vec![0u8; size * size * 4];
        super::mark(&mut pixels, size, [0xff, 0, 0]);
        let at = |x: usize, y: usize| &pixels[(y * size + x) * 4..][..4];
        assert_eq!(at(11, 11), [0, 0, 0xff, 0xff]);
        assert_eq!(at(1, 1), [0, 0, 0, 0]);
    }
}
