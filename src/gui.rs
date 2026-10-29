//! The window: a page each for the code, what the overlay may use, and what
//! happened. Changes save to pclink.ini at once; `service` picks them up.

use eframe::egui::{self, Color32, RichText};
use pclink::app::{self, Event, Status, modified};
use pclink::config::{self, Config, FileWatch, Folder, Program};
use pclink::files::Picker;
use pclink::run::Refused;
use pclink::tray::{self, Click, Mark};
use pclink::{autostart, journal, os, procs, run, service, toast};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const TITLE: &str = "Lazy Overlay PC Link";
const MAX_LOG: usize = 200;

// Colors.
const BG: Color32 = Color32::from_rgb(0x0e, 0x11, 0x1b);
const CARD: Color32 = Color32::from_rgb(0x18, 0x1d, 0x2c);
const WELL: Color32 = Color32::from_rgb(0x0b, 0x0e, 0x17);
const BTN: Color32 = Color32::from_rgb(0x23, 0x2a, 0x40);
const LINE: Color32 = Color32::from_rgb(0x2a, 0x31, 0x49);
const LINE_HI: Color32 = Color32::from_rgb(0x46, 0x50, 0x7a);
const INK: Color32 = Color32::from_rgb(0xe9, 0xec, 0xf5);
const GRAY: Color32 = Color32::from_rgb(0x98, 0xa2, 0xbb);
const FAINT: Color32 = Color32::from_rgb(0x6f, 0x78, 0x93);
const GREEN: Color32 = Color32::from_rgb(0x7c, 0xff, 0x9b);
const VIOLET: Color32 = Color32::from_rgb(0xa7, 0x7b, 0xff);
const YELLOW: Color32 = Color32::from_rgb(0xff, 0xb6, 0x38);
const RED: Color32 = Color32::from_rgb(0xff, 0x8f, 0x8f);
/// The header's line.
const RAINBOW: [Color32; 6] = [
    Color32::from_rgb(0xff, 0x00, 0x40),
    Color32::from_rgb(0xff, 0x95, 0x00),
    Color32::from_rgb(0xff, 0xe6, 0x00),
    Color32::from_rgb(0x35, 0xd5, 0x00),
    Color32::from_rgb(0x00, 0xc8, 0xff),
    Color32::from_rgb(0x7a, 0x3c, 0xff),
];

/// Always on Windows; with a display on Linux.
pub fn can_open() -> bool {
    cfg!(windows) || std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn options(size: [f32; 2], position: Option<(i32, i32)>) -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default().with_title(TITLE).with_inner_size(size).with_min_inner_size([900.0, 520.0]);
    if let Some((left, top)) = position.filter(|&(l, t)| os::on_screen(l, t)) {
        viewport = viewport.with_position([left as f32, top as f32]);
    }
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }
    eframe::NativeOptions { viewport, ..Default::default() }
}

/// What the connection thread tells the window.
#[derive(Default)]
struct Shared {
    status: Option<Status>,
    log: Vec<String>,
}

impl Shared {
    fn log(&mut self, line: String) {
        self.log.push(line);
        let over = self.log.len().saturating_sub(MAX_LOG);
        self.log.drain(..over);
    }
}

/// How the connection stands, for the icon by the clock.
fn tray_status(s: &Status) -> (Mark, String) {
    match s {
        Status::NoCode => (Mark::Problem, "Not set up yet".into()),
        Status::Connecting(_) => (Mark::Waiting, "Connecting...".into()),
        Status::Connected(login) => (Mark::Fine, format!("Connected to {login}'s overlay")),
        Status::Offline(_) => (Mark::Waiting, "Not connected. Trying again...".into()),
        Status::Revoked => (Mark::Problem, "The connection code no longer works".into()),
        Status::Replaced => (Mark::Problem, "Running on another computer".into()),
        Status::BadFile(_) => (Mark::Problem, "The settings file has a mistake".into()),
    }
}

/// Opens the window; closing it ends PC Link, or keeps it by the clock when
/// ticked.
pub fn open(path: PathBuf, minimized: bool) -> ! {
    // What it did before, from pclink.log.
    let shared = Arc::new(Mutex::new(Shared { status: None, log: journal::tail(&path, MAX_LOG) }));
    // Where it was left: its size, and its place while that is on a screen.
    let window = app::load(&path).map(|c| c.window).unwrap_or_default();
    let size = window.size.map_or([980.0, 640.0], |(w, h)| [w as f32, h as f32]);
    let result = eframe::run_native(
        TITLE,
        options(size, window.position),
        Box::new(move |cc| {
            style(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            // Closed by another program, or a signal: ends as when closed.
            #[cfg(windows)]
            {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Ok(h) = cc.window_handle()
                    && let RawWindowHandle::Win32(w) = h.as_raw()
                {
                    os::end_on_close(w.hwnd.get());
                }
            }
            #[cfg(unix)]
            {
                let c = ctx.clone();
                os::end_on_signal(move || c.request_repaint());
            }
            let (s, p, c) = (shared.clone(), path.clone(), ctx.clone());
            std::thread::spawn(move || {
                service::run(
                    p,
                    Arc::new(move |e: Event| {
                        if let Ok(mut sh) = s.lock() {
                            match e {
                                Event::Log(line) => sh.log(line),
                                Event::Status(st) => {
                                    let (mark, words) = tray_status(&st);
                                    tray::status(mark, &words);
                                    sh.status = Some(st);
                                }
                            }
                        }
                        c.request_repaint();
                    }),
                )
            });
            // The icon's clicks, and PC Link opened again, for the next frame.
            let clicked = Arc::new(Mutex::new(None));
            let last = clicked.clone();
            tray::listen(Arc::new(move |click| {
                if let Ok(mut l) = last.lock() {
                    *l = Some(click);
                }
                ctx.request_repaint();
            }));
            Ok(Box::new(Gui::new(path, shared, clicked, minimized)))
        }),
    );
    std::process::exit(if result.is_ok() { 0 } else { 1 })
}

/// For a second copy: PC Link is open already.
pub fn already_open() -> ! {
    let _ = eframe::run_native(
        TITLE,
        options([420.0, 180.0], None),
        Box::new(|cc| {
            style(&cc.egui_ctx);
            Ok(Box::new(Notice))
        }),
    );
    std::process::exit(0)
}

struct Notice;

impl eframe::App for Notice {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(12.0);
            ui.heading("PC Link is already open");
            ui.label("Look for its window on your taskbar, or its icon by the clock. Only one copy runs at a time.");
            ui.add_space(12.0);
            if ui.button("OK").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
}

// Always dark, the corners of a Windows app.
fn style(ctx: &egui::Context) {
    ctx.set_zoom_factor(1.1);
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut v = egui::Visuals::dark();
    (v.panel_fill, v.window_fill, v.extreme_bg_color, v.faint_bg_color) = (BG, CARD, WELL, CARD);
    (v.hyperlink_color, v.warn_fg_color, v.error_fg_color) = (VIOLET, YELLOW, RED);
    v.override_text_color = Some(INK);
    v.window_stroke = egui::Stroke::new(1.0, LINE);
    v.window_corner_radius = egui::CornerRadius::same(6);
    v.selection.bg_fill = VIOLET.gamma_multiply(0.45);
    v.selection.stroke = egui::Stroke::new(1.0, VIOLET);
    let w = &mut v.widgets;
    for (state, border) in [(&mut w.inactive, LINE), (&mut w.hovered, LINE_HI), (&mut w.active, VIOLET), (&mut w.open, LINE_HI)] {
        (state.bg_fill, state.weak_bg_fill) = (BTN, BTN);
        state.bg_stroke = egui::Stroke::new(1.0, border);
        state.fg_stroke = egui::Stroke::new(1.0, INK);
        state.corner_radius = egui::CornerRadius::same(4);
        state.expansion = 0.0;
    }
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0, LINE);
    w.noninteractive.fg_stroke = egui::Stroke::new(1.0, INK);
    w.noninteractive.corner_radius = egui::CornerRadius::same(4);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
        s.spacing.interact_size.y = 26.0;
    });
}

// A group of settings on a page.
fn group<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let r = egui::Frame::new()
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0, LINE))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner;
    ui.add_space(6.0);
    r
}

// Primary button.
fn primary(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).strong().color(BG)).fill(GREEN).stroke(egui::Stroke::new(1.0, GREEN)))
}

// Danger button.
fn danger(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(RED))
            .fill(Color32::TRANSPARENT)
            .stroke(egui::Stroke::new(1.0, RED.gamma_multiply(0.4))),
    )
}

// The rainbow line, the title and the version.
fn header(ui: &mut egui::Ui) {
    let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 3.0), egui::Sense::hover());
    let mut mesh = egui::Mesh::default();
    let step = line.width() / (RAINBOW.len() - 1) as f32;
    for (i, c) in RAINBOW.iter().enumerate() {
        let x = line.left() + step * i as f32;
        mesh.colored_vertex(egui::pos2(x, line.top()), *c);
        mesh.colored_vertex(egui::pos2(x, line.bottom()), *c);
        if i > 0 {
            let n = 2 * i as u32;
            mesh.add_triangle(n - 2, n - 1, n);
            mesh.add_triangle(n - 1, n, n + 1);
        }
    }
    ui.painter().add(mesh);
    egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 6)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(TITLE).size(16.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).color(FAINT));
            });
        });
    });
}

/// The pages, listed down the window's left side.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Page {
    Connection,
    Programs,
    Folders,
    Files,
    Settings,
    Activity,
}

struct Gui {
    path: PathBuf,
    cfg: Config,
    programs: Vec<Program>,
    folders: Vec<Folder>,
    filewatches: Vec<FileWatch>,
    /// Whether each watched path is a folder, asked once: the table is drawn
    /// every frame, and a network path can take a while to answer.
    dirs: std::collections::HashMap<std::path::PathBuf, bool>,
    shared: Arc<Mutex<Shared>>,
    /// The file could not be read: nothing is saved over it.
    broken: Option<Vec<String>>,
    page: Page,
    /// The program shown beside the list.
    chosen: Option<usize>,
    code: String,
    code_note: Option<(String, bool)>,
    /// Why the test notification could not show.
    toast_error: Option<String>,
    code_open: bool,
    picking: Option<Vec<PathBuf>>,
    dirty_since: Option<Instant>,
    save_error: Option<String>,
    /// When pclink.ini was last read or written here.
    stamp: Option<SystemTime>,
    looked: Instant,
    /// The status last drawn.
    seen: Option<Status>,
    /// The start-with-Windows command for this copy, and whether it is set.
    startup: String,
    starts: bool,
    startup_error: Option<String>,
    /// Minimize on the first frame (started with Windows, or ticked).
    minimize: bool,
    /// The icon's last click, or PC Link opened again, from the icon's thread.
    clicked: Arc<Mutex<Option<Click>>>,
    /// Hidden to the icon by the clock.
    hidden: bool,
    /// Minimized at the last frame: minimizing is seen as it happens.
    was_minimized: bool,
    /// When the activity was last copied.
    copied: Option<Instant>,
    /// The window's size and place as last seen, in logical pixels.
    place_size: Option<(u32, u32)>,
    place_position: Option<(i32, i32)>,
    /// Quitting for sure (the icon's menu, or answered): the close goes ahead.
    quitting: bool,
    /// Asking whether to close (ticked in the window).
    asking: bool,
}

impl Gui {
    fn new(path: PathBuf, shared: Arc<Mutex<Shared>>, clicked: Arc<Mutex<Option<Click>>>, minimize: bool) -> Self {
        let (cfg, broken) = match app::load(&path) {
            Ok(cfg) => (cfg, None),
            Err(errors) => (Config::default(), Some(errors)),
        };
        let programs = cfg.programs();
        let (folders, filewatches) = (cfg.folders.clone(), cfg.filewatches.clone());
        let code_open = cfg.key.is_none();
        let page = if code_open { Page::Connection } else { Page::Programs };
        let chosen = (!programs.is_empty()).then_some(0);
        let stamp = modified(&path);
        // The usual settings file is not named, as the installer writes it.
        let own = (config::settings_path().as_ref() != Some(&path)).then_some(path.as_path());
        let startup = std::env::current_exe().map(|exe| autostart::command(&exe, own)).unwrap_or_default();
        let starts = autostart::starts(autostart::saved().as_deref(), &startup);
        let minimize = minimize || cfg.window.start_minimized;
        Gui {
            path,
            cfg,
            programs,
            folders,
            filewatches,
            dirs: Default::default(),
            shared,
            broken,
            page,
            chosen,
            code: String::new(),
            code_note: None,
            toast_error: None,
            code_open,
            picking: None,
            dirty_since: None,
            save_error: None,
            stamp,
            looked: Instant::now(),
            seen: None,
            startup,
            starts,
            startup_error: None,
            minimize,
            clicked,
            hidden: false,
            was_minimized: false,
            copied: None,
            place_size: None,
            place_position: None,
            quitting: false,
            asking: false,
        }
    }

    // A hand edit while open, taken unless a change here waits to be saved.
    // One with mistakes is left to the status bar.
    fn follow_file(&mut self) {
        if self.dirty_since.is_some() || self.looked.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.looked = Instant::now();
        let now = modified(&self.path);
        if now == self.stamp {
            return;
        }
        self.stamp = now;
        if let Ok(cfg) = app::load(&self.path) {
            self.programs = cfg.programs();
            self.chosen = self.chosen.map(|i| i.min(self.programs.len().saturating_sub(1))).filter(|_| !self.programs.is_empty());
            (self.folders, self.filewatches) = (cfg.folders.clone(), cfg.filewatches.clone());
            self.dirs.clear();
            self.cfg = cfg;
            self.broken = None;
        }
    }

    // The window hidden, PC Link kept by the clock until its icon is clicked.
    fn hide_to_tray(&mut self, ctx: &egui::Context) {
        if self.dirty_since.is_some() {
            self.save();
        }
        self.hidden = true;
        tray::keep(true);
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    // Brought back and forward, from the icon or when PC Link is opened again.
    fn show_from_tray(&mut self, ctx: &egui::Context) {
        self.hidden = false;
        tray::keep(false);
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn changed(&mut self) {
        self.dirty_since = Some(Instant::now());
    }

    fn save(&mut self) {
        self.write(false);
    }

    // What the window has, keeping a hand edit of the key unless one was just
    // pasted or forgotten.
    fn write(&mut self, new_key: bool) {
        self.dirty_since = None;
        if self.broken.is_some() {
            return;
        }
        if !new_key
            && modified(&self.path) != self.stamp
            && let Ok(disk) = app::load(&self.path)
        {
            self.cfg.key = disk.key;
        }
        self.cfg.set_programs(&self.programs);
        self.cfg.set_folders(&self.folders);
        self.cfg.set_filewatches(&self.filewatches);
        self.save_error = config::save(&self.path, &self.cfg).err().map(|e| format!("Could not save your settings: {e}"));
        self.stamp = modified(&self.path);
    }

    fn use_code(&mut self, text: &str) {
        match config::parse_code(text) {
            Ok(key) => {
                self.cfg.key = Some(key);
                self.write(true);
                self.code.clear();
                self.code_open = false;
                self.code_note = Some(("Saved. Connecting...".into(), false));
            }
            Err(e) => self.code_note = Some((e, true)),
        }
    }

    fn paste_code(&mut self) {
        match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            // A code is under 200 characters: a page of text is not one, nor shown.
            Ok(text) if text.len() > 1000 => {
                self.code_note = Some(("That is not a PC Link connection code. Copy it again from your dashboard.".into(), true))
            }
            Ok(text) => {
                self.code = text.trim().to_string();
                let code = self.code.clone();
                self.use_code(&code);
            }
            Err(_) => self.code_note = Some(("Nothing to paste. Copy the code on your dashboard first.".into(), true)),
        }
    }

    // A program picked on this PC, shown; one listed already is listed again,
    // to start it another way.
    fn add(&mut self, path: PathBuf) {
        let program = Program::from_path(&path, &self.programs);
        self.push(program);
    }

    fn push(&mut self, program: Program) {
        self.programs.push(program);
        (self.chosen, self.page) = (Some(self.programs.len() - 1), Page::Programs);
        self.sort_programs();
        self.changed();
    }

    // By name, as the file is read; the chosen one stays chosen.
    fn sort_programs(&mut self) {
        let mut order: Vec<usize> = (0..self.programs.len()).collect();
        order.sort_by_cached_key(|&i| self.programs[i].name.to_lowercase());
        self.chosen = self.chosen.and_then(|c| order.iter().position(|&i| i == c));
        self.programs = order.iter().map(|&i| self.programs[i].clone()).collect();
    }

    // The connection in a word or two, its color, and a line more.
    fn status_words(&self) -> (Color32, String, String) {
        let status = self.shared.lock().ok().and_then(|s| s.status.clone());
        match &status {
            _ if self.broken.is_some() => (RED, "Your settings file has a mistake".into(), "See Connection.".into()),
            None | Some(Status::NoCode) => (GRAY, "Not set up yet".into(), "Paste your connection code to start.".into()),
            Some(Status::Connecting(host)) => (YELLOW, "Connecting...".into(), format!("to {host}")),
            Some(Status::Connected(login)) => {
                let keep = if self.cfg.window.close_to_tray && tray::supported() {
                    "You can close this window: PC Link keeps running by the clock."
                } else {
                    "Keep this window open while you stream. You can minimize it."
                };
                (GREEN, format!("Connected to {login}'s overlay"), keep.into())
            }
            Some(Status::Offline(why)) => (YELLOW, "Not connected. Trying again...".into(), friendly(why)),
            Some(Status::Revoked) => {
                (RED, "Your connection code no longer works".into(), "It was replaced on your dashboard. Paste the new one.".into())
            }
            Some(Status::Replaced) => (RED, "PC Link is running on another computer".into(), "Only one can be connected at a time.".into()),
            Some(Status::BadFile(_)) => (RED, "Your settings file has a mistake".into(), "See Connection.".into()),
        }
    }

    // A code that stops working opens the box for a new one, once, so Cancel
    // still closes it.
    fn watch_status(&mut self) {
        let status = self.shared.lock().ok().and_then(|s| s.status.clone());
        if status != self.seen {
            if status == Some(Status::Revoked) {
                (self.code_open, self.page) = (true, Page::Connection);
            }
            self.seen = status;
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let (color, title, detail) = self.status_words();
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 6.0, color.gamma_multiply(0.25));
            ui.painter().circle_filled(rect.center(), 3.5, color);
            ui.label(RichText::new(title).strong());
            ui.label(RichText::new(detail).color(GRAY));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(e) = &self.save_error {
                    ui.colored_label(RED, e);
                } else if self.unsaved() {
                    ui.colored_label(YELLOW, "Something in red is not saved until it is fixed");
                } else if self.dirty_since.is_none() && self.broken.is_none() {
                    ui.label(RichText::new("All changes saved").color(FAINT));
                }
            });
        });
    }

    // Whether something here is left out of the file: a name, or what a
    // ticked program needs, or a path the file can't hold.
    fn unsaved(&self) -> bool {
        let any = |names: Vec<&str>| (0..names.len()).any(|i| !config::kept_name(&names, i));
        any(self.programs.iter().map(|p| p.name.as_str()).collect())
            || any(self.folders.iter().map(|f| f.name.as_str()).collect())
            || any(self.filewatches.iter().map(|f| f.name.as_str()).collect())
            || self.programs.iter().any(program_unsaved)
            || self.folders.iter().any(|f| !config::valid_path(&f.path))
            || self.filewatches.iter().any(|f| !config::valid_path(&f.path))
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        let count = |n: usize| if n == 0 { String::new() } else { format!("  ({n})") };
        let pages = [
            (Page::Connection, "Connection".to_string()),
            (Page::Programs, format!("Programs{}", count(self.programs.len()))),
            (Page::Folders, format!("Random-file folders{}", count(self.folders.len()))),
            (Page::Files, format!("Watched files{}", count(self.filewatches.len()))),
            (Page::Settings, "Settings".to_string()),
            (Page::Activity, "Activity".to_string()),
        ];
        ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
            for (page, label) in pages {
                let r = ui.add(egui::Button::selectable(self.page == page, label).min_size(egui::vec2(0.0, 30.0)));
                if r.clicked() {
                    self.page = page;
                }
            }
        });
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.label(
                RichText::new("PC Link only does what you allow here. Your overlay can't change these settings.").small().color(FAINT),
            );
        });
    }

    fn connection_page(&mut self, ui: &mut egui::Ui) {
        page_title(ui, "Connection", "PC Link connects this PC to your overlay, with the code from your dashboard.");
        let (color, title, detail) = self.status_words();
        let replaced = self.shared.lock().ok().and_then(|s| s.status.clone()) == Some(Status::Replaced);
        group(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 9.0, color.gamma_multiply(0.25));
                ui.painter().circle_filled(rect.center(), 5.0, color);
                ui.label(RichText::new(title).size(17.0).strong());
            });
            ui.label(RichText::new(detail).color(GRAY));
            if replaced && primary(ui, "Use PC Link on this computer").clicked() {
                self.save();
            }
        });
        self.broken_box(ui);
        if self.broken.is_some() {
            return;
        }
        group(ui, |ui| {
            if !self.code_open {
                ui.horizontal(|ui| {
                    ui.label("Connection code saved.");
                    if ui.button("Change...").clicked() {
                        self.code_open = true;
                    }
                    if ui.button("Forget").on_hover_text("Takes it off this PC; paste it again to connect").clicked() {
                        self.cfg.key = None;
                        self.write(true);
                        self.code_open = true;
                        self.code_note = Some(("Forgotten: PC Link is not connected. Paste a code to connect again.".into(), false));
                    }
                });
            } else {
                heading(ui, "Connect to your overlay");
                ui.label("1.  On your dashboard, open Integrations, then PC Link.");
                ui.label("2.  Click Get connection code, then Copy.");
                ui.label("3.  Come back here and click Paste.");
                ui.horizontal(|ui| {
                    if primary(ui, "Paste connection code").clicked() {
                        self.paste_code();
                    }
                    if self.cfg.key.is_some() && ui.button("Cancel").clicked() {
                        self.code_open = false;
                        self.code_note = None;
                    }
                });
                ui.horizontal(|ui| {
                    let field = ui.add(egui::TextEdit::singleline(&mut self.code).hint_text("or paste it here").desired_width(360.0));
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui.button("Connect").clicked() || enter) && !self.code.trim().is_empty() {
                        let code = self.code.clone();
                        self.use_code(&code);
                    }
                });
            }
            if let Some((note, error)) = &self.code_note {
                ui.colored_label(if *error { RED } else { GREEN }, note);
            }
        });
    }

    fn pick_program(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title("Choose a program for your overlay");
        if cfg!(windows) {
            dialog = dialog.add_filter("Programs", &["exe"]);
        }
        if let Some(path) = dialog.pick_file() {
            self.add(path);
        }
    }

    // The list on the left, the chosen one's settings on the right.
    fn programs_page(&mut self, ui: &mut egui::Ui) {
        page_title(
            ui,
            "Programs",
            "Your overlay can only start, close or watch what is listed here. List a program again to start it with other arguments.",
        );
        let chosen = self.chosen.filter(|&i| i < self.programs.len());
        let room = self.programs.len() < config::MAX_ENTRIES;
        let full = format!("At most {} programs", config::MAX_ENTRIES);
        ui.horizontal(|ui| {
            if ui.add_enabled(room, egui::Button::new("Add a program...")).on_disabled_hover_text(&full).clicked() {
                self.pick_program();
            }
            if ui.add_enabled(room, egui::Button::new("Pick one that is open now...")).on_disabled_hover_text(&full).clicked() {
                self.picking = Some(procs::open_programs());
            }
            ui.separator();
            let can_copy = room && chosen.is_some_and(|i| self.programs[i].path.is_some());
            let r = ui
                .add_enabled(can_copy, egui::Button::new("Duplicate entry"))
                .on_hover_text("The same program again, under its own name, to start it with other arguments")
                .on_disabled_hover_text(if room { "Choose a program that PC Link starts first" } else { &full });
            if r.clicked()
                && let Some(i) = chosen
            {
                let another = self.programs[i].another(&self.programs);
                self.push(another);
            }
            if ui.add_enabled(chosen.is_some(), egui::Button::new("Remove")).clicked()
                && let Some(i) = chosen
            {
                self.programs.remove(i);
                self.chosen = (!self.programs.is_empty()).then(|| i.min(self.programs.len() - 1));
                self.changed();
            }
        });
        ui.add_space(4.0);
        let list = egui::Frame::new().fill(WELL).stroke(egui::Stroke::new(1.0, LINE)).corner_radius(4).inner_margin(4);
        egui::Panel::left("program list")
            .resizable(true)
            .default_size(250.0)
            .size_range(200.0..=440.0)
            .frame(list)
            .show(ui, |ui| self.program_list(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(egui::Margin { left: 14, ..Default::default() })).show(
            ui,
            |ui| {
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| self.program_form(ui));
            },
        );
    }

    fn program_list(&mut self, ui: &mut egui::Ui) {
        let mut clicked = false;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if self.programs.is_empty() {
                ui.label(RichText::new("Nothing yet. Add the programs you want your stream to start, close or react to.").color(GRAY));
            }
            let names: Vec<&str> = self.programs.iter().map(|p| p.name.as_str()).collect();
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                for (i, p) in self.programs.iter().enumerate() {
                    let mut job = egui::text::LayoutJob::default();
                    let name_color = if config::kept_name(&names, i) && !program_unsaved(p) { INK } else { RED };
                    let format = |size: f32, color: Color32| egui::TextFormat::simple(egui::FontId::proportional(size), color);
                    job.append(&p.name, 0.0, format(14.0, name_color));
                    let does: Vec<&str> =
                        [(p.start, "starts"), (p.stop, "closes"), (p.watch, "watches")].iter().filter(|d| d.0).map(|d| d.1).collect();
                    let does = if does.is_empty() { "does nothing yet".to_string() } else { does.join(", ") };
                    job.append(&format!("   {does}\n"), 0.0, format(11.0, FAINT));
                    let file = p.path.as_ref().and_then(|x| x.file_name()).map(|f| f.to_string_lossy().to_string());
                    let mut line = file.unwrap_or_else(|| p.process.clone());
                    for a in p.args.iter().filter(|a| !a.trim().is_empty()) {
                        line += " ";
                        line += a.trim();
                    }
                    job.append(&line, 0.0, format(11.5, GRAY));
                    if ui.add(egui::Button::selectable(self.chosen == Some(i), job)).clicked() {
                        (self.chosen, clicked) = (Some(i), true);
                    }
                }
            });
        });
        // A name just typed in, put in its place.
        if clicked {
            self.sort_programs();
        }
    }

    // Everything the file holds for the chosen program.
    fn program_form(&mut self, ui: &mut egui::Ui) {
        let Some(i) = self.chosen.filter(|&i| i < self.programs.len()) else {
            ui.label(RichText::new("Choose a program on the left, or add one.").color(GRAY));
            return;
        };
        let shared = self.shared.clone();
        let same: Vec<String> = self
            .programs
            .iter()
            .enumerate()
            .filter(|&(n, q)| n != i && q.path.is_some() && q.path == self.programs[i].path)
            .map(|(_, q)| q.name.clone())
            .collect();
        let earlier: Vec<String> = self.programs[..i].iter().map(|q| config::slug(&q.name)).collect();
        let p = &mut self.programs[i];
        let (mut changed, mut resort) = (false, false);
        row(ui, "Name", |ui| {
            let (ok, taken) = (config::valid_name(&p.name), earlier.contains(&config::slug(&p.name)));
            let mut edit = egui::TextEdit::singleline(&mut p.name).desired_width(260.0);
            if !ok || taken {
                edit = edit.text_color(RED);
            }
            let r = ui.add(edit).on_hover_text("The name your actions use for it");
            // Put in its place once typed, not at each key.
            (changed, resort) = (changed | r.changed(), r.lost_focus());
            if !ok {
                ui.colored_label(RED, "Use 1 to 30 letters, numbers, spaces, - or _. Not saved until it fits.");
            } else if taken {
                ui.colored_label(RED, "Another program above has this name. Not saved until it differs.");
            }
        });
        row(ui, "Program", |ui| {
            match &p.path {
                Some(path) => ui.label(path.display().to_string()),
                None => ui.label(RichText::new("Not started by PC Link (listed by its process)").color(GRAY)),
            };
            if !same.is_empty() {
                ui.label(RichText::new(format!("Also listed as: {}", same.join(", "))).small().color(FAINT));
            }
        });
        row(ui, "Its process", |ui| {
            changed |= ui
                .add(egui::TextEdit::singleline(&mut p.process).desired_width(260.0).hint_text("obs64.exe"))
                .on_hover_text("The file name it runs as, to close or watch it. A launcher's game may run as another.")
                .changed();
            if !config::valid_process(&p.process) && (p.stop || p.watch) {
                ui.colored_label(RED, "Its file name, like obs64.exe, not a path. Not closed or watched until it fits.");
            }
        });
        row(ui, "Your overlay may", |ui| {
            changed |= ui
                .add_enabled(p.path.is_some(), egui::Checkbox::new(&mut p.start, "Start it"))
                .on_disabled_hover_text("Remove it and add it again with Add a program to let your overlay start it.")
                .changed();
            changed |= ui.checkbox(&mut p.stop, "Close it").changed();
            changed |= ui.checkbox(&mut p.watch, "Be told when it opens or closes").changed();
        });
        if p.start && p.as_start().is_none() {
            row(ui, "", |ui| {
                ui.colored_label(RED, "Not started until fixed: its path, an argument or its folder has a character the file can't hold.");
            });
        }
        if p.path.is_some() {
            row(ui, "Arguments", |ui| {
                let mut drop = None;
                for (n, arg) in p.args.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        changed |= ui.add(egui::TextEdit::singleline(arg).desired_width(260.0)).changed();
                        if ui.small_button("Remove").clicked() {
                            drop = Some(n);
                        }
                    });
                }
                if let Some(n) = drop {
                    p.args.remove(n);
                    changed = true;
                }
                if p.args.len() < 20 && ui.small_button("Add an argument").on_hover_text("One each, like --minimized").clicked() {
                    p.args.push(String::new());
                    changed = true;
                }
            });
            row(ui, "Starts in", |ui| {
                ui.horizontal_wrapped(|ui| {
                    let dir = p.dir.as_ref().map_or("its own folder".to_string(), |d| d.display().to_string());
                    ui.label(dir);
                    if ui.small_button("Choose...").clicked()
                        && let Some(d) = rfd::FileDialog::new().set_title("Choose the folder it starts in").pick_folder()
                    {
                        p.dir = Some(d);
                        changed = true;
                    }
                    if p.dir.is_some() && ui.small_button("Use its own").clicked() {
                        p.dir = None;
                        changed = true;
                    }
                });
            });
            row(ui, "Starting", |ui| {
                changed |= ui.checkbox(&mut p.once, "Don't start it again while it is running").changed();
                if cfg!(windows) {
                    changed |= ui.checkbox(&mut p.hidden, "Start it with no window (for console programs)").changed();
                }
            });
        }
        row(ui, "Closing", |ui| {
            changed |= ui.checkbox(&mut p.force, "Close it by force (it can't save or ask first)").changed();
        });
        row(ui, "Try it", |ui| {
            ui.horizontal(|ui| {
                let start = p.as_start();
                let r = ui.add_enabled(start.is_some(), egui::Button::new("Start it now"));
                if r.on_hover_text("Starts it here, as your overlay would").clicked()
                    && let Some(s) = start
                {
                    try_it(&shared, ui.ctx(), s.name.clone(), move || run::start(&s));
                }
                let stop = p.as_stop();
                let r = ui.add_enabled(stop.is_some(), egui::Button::new("Close it now"));
                if r.on_hover_text("Closes it here, as your overlay would").clicked()
                    && let Some(s) = stop
                {
                    try_it(&shared, ui.ctx(), s.name.clone(), move || run::stop(&s));
                }
            });
        });
        if resort {
            self.sort_programs();
        }
        if changed {
            self.changed();
        }
    }

    // A name for a file or folder picked on this PC.
    fn name_for(path: &std::path::Path, taken: Vec<&str>) -> String {
        let stem = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        config::free_name(&stem, &taken)
    }

    fn folders_page(&mut self, ui: &mut egui::Ui) {
        page_title(ui, "Random-file folders", "A Random file step names one; the file it picks is the only thing your overlay learns.");
        let room = self.folders.len() < config::MAX_ENTRIES;
        let full = format!("At most {} folders", config::MAX_ENTRIES);
        if ui.add_enabled(room, egui::Button::new("Add a folder...")).on_disabled_hover_text(&full).clicked()
            && let Some(path) = rfd::FileDialog::new().set_title("Choose a folder for your overlay").pick_folder()
            && !self.folders.iter().any(|f| f.path == path)
        {
            let name = Self::name_for(&path, self.folders.iter().map(|f| f.name.as_str()).collect());
            self.folders.push(Folder { name, path, subfolders: false });
            self.changed();
        }
        ui.add_space(4.0);
        let mut remove = None;
        let mut changed = false;
        let shared = self.shared.clone();
        let names: Vec<String> = self.folders.iter().map(|f| f.name.clone()).collect();
        table(ui, "folders", &["Name", "Folder", "Subfolders", "", ""], self.folders.is_empty(), |ui| {
            for (i, f) in self.folders.iter_mut().enumerate() {
                changed |= name_cell(ui, &mut f.name, kept(&names, i));
                path_cell(ui, &f.path);
                changed |= ui.checkbox(&mut f.subfolders, "Include").changed();
                let r = ui.button("Pick one now");
                if r.on_hover_text("Picks a file from it, as a Random file step would").clicked() {
                    let folder = f.clone();
                    try_it(&shared, ui.ctx(), f.name.clone(), move || {
                        Picker::default().pick(&folder, "").map(|p| format!("picked {}", p.display())).map_err(Refused::same)
                    });
                }
                if danger(ui, "Remove").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        name_rule(ui, &names);
        if let Some(i) = remove {
            self.folders.remove(i);
            changed = true;
        }
        if changed {
            self.changed();
        }
    }

    fn files_page(&mut self, ui: &mut egui::Ui) {
        page_title(ui, "Watched files", "When a file here is created, changed, deleted or renamed, your actions can react, with its path.");
        ui.horizontal(|ui| {
            let mut picked = None;
            let room = self.filewatches.len() < config::MAX_ENTRIES;
            let full = format!("At most {} watched files and folders", config::MAX_ENTRIES);
            if ui.add_enabled(room, egui::Button::new("Watch a folder...")).on_disabled_hover_text(&full).clicked() {
                picked = rfd::FileDialog::new().set_title("Choose a folder to watch").pick_folder();
            }
            if ui.add_enabled(room, egui::Button::new("Watch a file...")).on_disabled_hover_text(&full).clicked() {
                picked = rfd::FileDialog::new().set_title("Choose a file to watch").pick_file();
            }
            if let Some(path) = picked.filter(|p| !self.filewatches.iter().any(|f| &f.path == p)) {
                let name = Self::name_for(&path, self.filewatches.iter().map(|f| f.name.as_str()).collect());
                self.filewatches.push(FileWatch { name, path, ..Default::default() });
                self.changed();
            }
        });
        ui.add_space(4.0);
        let mut remove = None;
        let mut changed = false;
        let columns = ["Name", "File or folder", "Subfolders", "Only names with", ""];
        let names: Vec<String> = self.filewatches.iter().map(|f| f.name.clone()).collect();
        table(ui, "filewatches", &columns, self.filewatches.is_empty(), |ui| {
            for (i, f) in self.filewatches.iter_mut().enumerate() {
                changed |= name_cell(ui, &mut f.name, kept(&names, i));
                path_cell(ui, &f.path);
                if *self.dirs.entry(f.path.clone()).or_insert_with(|| f.path.is_dir()) {
                    changed |= ui.checkbox(&mut f.subfolders, "Include").changed();
                    let field = egui::TextEdit::singleline(&mut f.matching).desired_width(110.0).char_limit(config::MAX_MATCH);
                    changed |= ui.add_sized([110.0, 24.0], field.hint_text("any, or .mp4")).changed();
                } else {
                    ui.label("");
                    ui.label("");
                }
                if danger(ui, "Remove").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        name_rule(ui, &names);
        if let Some(i) = remove {
            self.filewatches.remove(i);
            changed = true;
        }
        if changed {
            self.changed();
        }
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        page_title(ui, "Settings", "How PC Link shows notifications, how much your overlay may ask, and how it starts and closes.");
        ui.columns(2, |cols| {
            group(&mut cols[0], |ui| self.toasts_box(ui));
            group(&mut cols[0], |ui| self.limit_box(ui));
            group(&mut cols[1], |ui| self.window_box(ui));
        });
        ui.add_space(4.0);
        ui.label(RichText::new(format!("Settings file: {}", self.path.display())).small().color(FAINT));
    }

    fn toasts_box(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Notifications on this PC");
        let mut changed = ui.checkbox(&mut self.cfg.toasts.actions, "Show the ones my actions send").changed();
        changed |= ui.checkbox(&mut self.cfg.toasts.errors, "Show one when something on my overlay fails").changed();
        if changed {
            self.changed();
        }
        if ui.button("Show a test notification").clicked() {
            self.toast_error = toast::show("Lazy Overlay PC Link", "This is how your overlay's notifications look.", false)
                .err()
                .map(|e| format!("It could not be shown: {e}"));
        }
        if let Some(e) = &self.toast_error {
            ui.colored_label(RED, e);
        }
    }

    fn limit_box(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Limit");
        let slider = egui::Slider::new(&mut self.cfg.runs_per_minute, 1..=120).text("a minute, at most");
        if ui.add(slider).on_hover_text("However many your overlay asks for").changed() {
            self.changed();
        }
        ui.label(RichText::new("Starts, closes and picks your overlay may ask for.").small().color(GRAY));
    }

    // Starting with Windows (off unless ticked here or in the installer), and
    // closing to the icon by the clock.
    fn window_box(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Starting and closing");
        let w = &mut self.cfg.window;
        let mut changed = false;
        if tray::supported() {
            let hint = "Click its icon to open this window again; right-click it to quit";
            changed |= ui
                .checkbox(&mut w.close_to_tray, "Keep PC Link running by the clock when I close this window")
                .on_hover_text(hint)
                .changed();
            changed |= ui.checkbox(&mut w.minimize_to_tray, "Minimize it to the clock too").on_hover_text(hint).changed();
        }
        changed |= ui.checkbox(&mut w.confirm_close, "Ask before closing PC Link").changed();
        let hint = if tray::supported() && (w.close_to_tray || w.minimize_to_tray) { "To its icon by the clock" } else { "To the taskbar" };
        changed |= ui.checkbox(&mut w.start_minimized, "Start PC Link minimized").on_hover_text(hint).changed();
        if changed {
            self.changed();
        }
        if !autostart::supported() || self.startup.is_empty() {
            return;
        }
        let mut on = self.starts;
        if ui.checkbox(&mut on, "Start PC Link when Windows starts").on_hover_text("Minimized, ready for your stream").changed() {
            match autostart::set(on.then_some(self.startup.as_str())) {
                Ok(()) => (self.starts, self.startup_error) = (on, None),
                Err(e) => self.startup_error = Some(format!("Could not change that: {e}")),
            }
        }
        if let Some(e) = &self.startup_error {
            ui.colored_label(RED, e);
        }
    }

    // Are you sure: PC Link ends, and the overlay loses this PC until it opens again.
    fn ask_close(&mut self, ctx: &egui::Context) {
        if !self.asking {
            return;
        }
        let modal = egui::Modal::new(egui::Id::unique("ask_close")).show(ctx, |ui| {
            ui.set_width(340.0);
            heading(ui, "Close PC Link?");
            ui.label(RichText::new("Your overlay can't use this PC until PC Link is open again.").color(GRAY));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if danger(ui, "Close PC Link").clicked() {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if ui.button("Cancel").clicked() {
                    self.asking = false;
                }
            });
        });
        if modal.should_close() {
            self.asking = false;
        }
    }

    fn picker(&mut self, ctx: &egui::Context) {
        let Some(list) = self.picking.clone() else { return };
        let mut chosen = None;
        let mut close = false;
        egui::Window::new("Programs open now").collapsible(false).resizable(true).default_size([440.0, 420.0]).show(ctx, |ui| {
            ui.label("Click the one to add.");
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                if list.is_empty() {
                    ui.label("No programs found. Use Add a program instead.");
                }
                for path in &list {
                    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    let r = ui.add(egui::Button::new(name).frame(false)).on_hover_text(path.display().to_string());
                    if r.clicked() {
                        chosen = Some(path.clone());
                    }
                }
            });
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
        if let Some(path) = chosen {
            self.add(path);
            close = true;
        }
        if close {
            self.picking = None;
        }
    }

    fn broken_box(&mut self, ui: &mut egui::Ui) {
        let errors = match (&self.broken, self.shared.lock().ok().and_then(|s| s.status.clone())) {
            (Some(e), _) => e.clone(),
            (None, Some(Status::BadFile(e))) => e,
            _ => return,
        };
        group(ui, |ui| {
            ui.colored_label(RED, format!("{} could not be read:", self.path.display()));
            for e in errors.iter().take(5) {
                ui.label(format!("  {e}"));
            }
            ui.label("Fix it by hand, or start fresh (a copy of the old file is kept beside it).");
            if self.broken.is_some() && primary(ui, "Start fresh").clicked() {
                // Never written over: kept as .ini.old first, or left as it is.
                match std::fs::rename(&self.path, self.path.with_extension("ini.old")) {
                    Ok(()) => {
                        self.broken = None;
                        self.cfg = Config::default();
                        self.programs.clear();
                        self.chosen = None;
                        self.folders.clear();
                        self.filewatches.clear();
                        self.code_open = true;
                        self.write(true);
                    }
                    Err(e) => self.save_error = Some(format!("Could not keep a copy of the old file, so it was left as it is: {e}")),
                }
            }
        });
    }

    fn activity_page(&mut self, ui: &mut egui::Ui) {
        page_title(ui, "Activity", "What PC Link did, newest first. It is kept in pclink.log beside the settings file.");
        let log = self.shared.lock().map(|s| s.log.clone()).unwrap_or_default();
        ui.horizontal(|ui| {
            if ui.add_enabled(!log.is_empty(), egui::Button::new("Copy all")).on_hover_text("To paste it to someone helping you").clicked()
            {
                let text = log.join(if cfg!(windows) { "\r\n" } else { "\n" });
                self.copied = arboard::Clipboard::new().and_then(|mut c| c.set_text(text)).ok().map(|_| Instant::now());
            }
            if self.copied.is_some_and(|t| t.elapsed() < Duration::from_secs(3)) {
                ui.label(RichText::new("Copied.").color(GREEN));
            }
        });
        ui.add_space(4.0);
        let frame = egui::Frame::new().fill(WELL).stroke(egui::Stroke::new(1.0, LINE)).corner_radius(4).inner_margin(8);
        frame.show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                if log.is_empty() {
                    ui.label(RichText::new("Nothing yet.").color(GRAY));
                }
                // Today's without their date.
                let today = format!("{} ", &os::local_time()[..10]);
                for line in log.iter().rev() {
                    ui.label(RichText::new(line.strip_prefix(&today).unwrap_or(line)).monospace().size(12.0));
                }
            });
        });
    }
}

// A program started or closed from the window, off its thread; what it did
// goes in the activity.
fn try_it(shared: &Arc<Mutex<Shared>>, ctx: &egui::Context, name: String, act: impl FnOnce() -> Result<String, Refused> + Send + 'static) {
    let (shared, ctx) = (shared.clone(), ctx.clone());
    std::thread::spawn(move || {
        let line = journal::stamp(&match act() {
            Ok(done) => format!("You tried {name}: {done}."),
            Err(r) => format!("You tried {name}: {}.", r.why),
        });
        journal::keep(&line);
        if let Ok(mut s) = shared.lock() {
            s.log(line);
        }
        ctx.request_repaint();
    });
}

// A section's heading.
fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(15.0).strong());
}

// A page's title and what it is for.
fn page_title(ui: &mut egui::Ui, title: &str, about: &str) {
    ui.label(RichText::new(title).size(20.0).strong());
    ui.label(RichText::new(about).color(GRAY));
    ui.add_space(6.0);
}

// Ticked to start, close or watch, without what that needs.
fn program_unsaved(p: &Program) -> bool {
    (p.start && p.as_start().is_none()) || ((p.stop || p.watch) && !config::valid_process(&p.process))
}

// A path in a table, red when the file can't hold it.
fn path_cell(ui: &mut egui::Ui, path: &std::path::Path) {
    let text = path.display().to_string();
    if config::valid_path(path) {
        ui.label(text);
    } else {
        ui.colored_label(RED, text).on_hover_text("Not saved: the file can't hold this path");
    }
}

// One setting of a form: its label, then what it holds, top to bottom.
fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(130.0, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_size(egui::vec2(130.0, 24.0));
            ui.label(RichText::new(label).color(GRAY));
        });
        ui.vertical(add);
    });
    ui.add_space(4.0);
}

// Rows in columns under their headings, striped, in a frame.
fn table(ui: &mut egui::Ui, id: &str, columns: &[&str], empty: bool, rows: impl FnOnce(&mut egui::Ui)) {
    let frame = egui::Frame::new().fill(WELL).stroke(egui::Stroke::new(1.0, LINE)).corner_radius(4).inner_margin(8);
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::both().auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new(id).num_columns(columns.len()).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
                for c in columns {
                    ui.label(RichText::new(*c).small().strong().color(GRAY));
                }
                ui.end_row();
                rows(ui);
            });
            if empty {
                ui.label(RichText::new("Nothing yet.").color(GRAY));
            }
        });
    });
}

// Whether the `i`th of a table's names is saved.
fn kept(names: &[String], i: usize) -> bool {
    config::kept_name(&names.iter().map(String::as_str).collect::<Vec<_>>(), i)
}

// An entry's name in a table, editable, red unless `ok`; whether it changed.
fn name_cell(ui: &mut egui::Ui, name: &mut String, ok: bool) -> bool {
    let mut edit = egui::TextEdit::singleline(name).desired_width(170.0);
    if !ok {
        edit = edit.text_color(RED);
    }
    ui.add_sized([170.0, 24.0], edit).on_hover_text("The name your actions use for it").changed()
}

// Said once under a table with a name that is not saved.
fn name_rule(ui: &mut egui::Ui, names: &[String]) {
    if (0..names.len()).any(|i| !kept(names, i)) {
        ui.colored_label(RED, "Names use 1 to 30 letters, numbers, spaces, - or _, each its own. One in red is not saved until it fits.");
    }
}

/// A connection problem in plain words.
fn friendly(why: &str) -> String {
    let w = why.to_ascii_lowercase();
    if w.contains("not found") || w.contains("could not reach") || w.contains("resolve") || w.contains("timed out") || w.contains("in time")
    {
        "Your overlay's server can't be reached. Check your internet connection.".into()
    } else if w.contains("certificate") {
        "The server's security certificate was not trusted. Check your PC's date and time.".into()
    } else if w.contains("openssl") {
        "OpenSSL is needed: install it with your system's package manager.".into()
    } else {
        format!("({why})")
    }
}

impl eframe::App for Gui {
    // Runs while the window is hidden too: the icon's clicks, and the close.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let w = self.cfg.window;
        let tray = w.close_to_tray && tray::supported();
        if std::mem::take(&mut self.minimize) {
            if (w.close_to_tray || w.minimize_to_tray) && tray::supported() {
                self.hide_to_tray(ctx);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        }
        let click = self.clicked.lock().ok().and_then(|mut c| c.take());
        // Asked from outside (a script, Task Manager, a signal): ends now,
        // not kept by the clock and without asking.
        if (os::ending() || click == Some(Click::End)) && !self.quitting {
            self.quitting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        match click {
            Some(Click::Open) if self.quitting => {}
            Some(Click::Open) => self.show_from_tray(ctx),
            // Asked in the window, brought back for it.
            Some(Click::Quit) if w.confirm_close => {
                self.show_from_tray(ctx);
                self.asking = true;
            }
            Some(Click::Quit) => {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Some(Click::End) | None => {}
        }
        if !self.quitting && ctx.input(|i| i.viewport().close_requested()) {
            if tray {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide_to_tray(ctx);
            } else if w.confirm_close {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.asking = true;
            }
        }
        let (minimized, place) = ctx.input(|i| {
            let v = i.viewport();
            (v.minimized == Some(true), v.inner_rect.zip(v.outer_rect))
        });
        if minimized && !self.was_minimized && !self.hidden && w.minimize_to_tray && tray::supported() {
            self.hide_to_tray(ctx);
        }
        self.was_minimized = minimized;
        // Where it is, while shown as it is: kept when PC Link ends.
        if let Some((inner, outer)) = place.filter(|_| !minimized && !self.hidden) {
            let zoom = ctx.zoom_factor();
            let size = ((inner.width() * zoom).round() as u32, (inner.height() * zoom).round() as u32);
            let position = ((outer.left() * zoom).round() as i32, (outer.top() * zoom).round() as i32);
            if size.0 >= 200 && size.1 >= 200 {
                (self.place_size, self.place_position) = (Some(size), Some(position));
            }
        }
    }

    // A Windows-style window: the pages down the left, the one open beside
    // them, the connection along the bottom.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.dirty_since.is_some_and(|t| t.elapsed() >= Duration::from_millis(400)) {
            self.save();
        } else if self.dirty_since.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(450));
        }
        self.follow_file();
        self.watch_status();
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        let ctx = ui.ctx().clone();
        egui::Panel::top("header").frame(egui::Frame::new().fill(BG)).show(ui, header);
        let bar = egui::Frame::new().fill(CARD).stroke(egui::Stroke::new(1.0, LINE)).inner_margin(egui::Margin::symmetric(12, 5));
        egui::Panel::bottom("status bar").frame(bar).show(ui, |ui| self.status_bar(ui));
        let side = egui::Frame::new().fill(CARD).stroke(egui::Stroke::new(1.0, LINE)).inner_margin(8);
        egui::Panel::left("pages").resizable(false).exact_size(210.0).frame(side).show(ui, |ui| self.nav(ui));
        let main = egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(18, 14));
        egui::CentralPanel::default().frame(main).show(ui, |ui| self.page(ui));
        self.picker(&ctx);
        self.ask_close(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let w = &mut self.cfg.window;
        if self.place_size.is_some() && (w.size, w.position) != (self.place_size, self.place_position) {
            (w.size, w.position) = (self.place_size, self.place_position);
            self.dirty_since = Some(Instant::now());
        }
        if self.dirty_since.is_some() {
            self.save();
        }
        tray::end();
    }
}

impl Gui {
    // The page open; while the file can't be read, only how to fix it.
    fn page(&mut self, ui: &mut egui::Ui) {
        if self.broken.is_some() && !matches!(self.page, Page::Connection | Page::Activity) {
            self.page = Page::Connection;
        }
        match self.page {
            Page::Connection => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.connection_page(ui));
            }
            Page::Programs => self.programs_page(ui),
            Page::Folders => self.folders_page(ui),
            Page::Files => self.files_page(ui),
            Page::Settings => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.settings_page(ui));
            }
            Page::Activity => self.activity_page(ui),
        }
    }
}
