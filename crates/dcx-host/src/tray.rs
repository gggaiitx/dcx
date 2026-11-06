//! System tray icon + context menu, plus the listener that turns menu clicks
//! into "open settings window" / "quit" actions.
//!
//! We use the global `MenuEvent` channel (from muda, re-exported by tray-icon)
//! rather than the `with_on_menu_event` closure so ownership of the `Menu` /
//! `MenuItem`s stays unambiguous: the `TrayIcon` returned by `build_tray` keeps
//! them alive, and a dedicated listener thread drains the event channel.

use std::sync::atomic::Ordering;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

/// Build the tray icon and start the event listener. The caller must keep the
/// returned `TrayIcon` alive (it owns the menu and the OS tray entry).
pub fn build_tray() -> tray_icon::TrayIcon {
    let open = MenuItem::new("设置 (Settings)", true, None);
    let box_item = MenuItem::new("桌面整理 (File box)", true, None);
    let restore = MenuItem::new("退出并恢复任务栏 (Quit & restore taskbar)", true, None);
    let quit = MenuItem::new("退出 (Quit)", true, None);

    let menu = Menu::new();
    menu.append_items(&[&open, &box_item, &restore, &quit])
        .expect("append menu items");

    let open_id = open.id().clone();
    let box_id = box_item.id().clone();
    let restore_id = restore.id().clone();
    let quit_id = quit.id().clone();

    let icon = make_icon();

    let tray = TrayIconBuilder::new()
        .with_tooltip("DCX 桌面增强")
        .with_menu(Box::new(menu))
        // tray-icon defaults to opening the menu on *left* click. Keep the
        // familiar split instead: left click brings up the window, right click
        // opens the menu.
        .with_menu_on_left_click(false)
        .with_icon(icon)
        .build()
        .expect("create tray icon");

    // Listener thread: the channel is populated by tray-icon's internal thread.
    std::thread::spawn(move || {
        let receiver = MenuEvent::receiver();
        for event in receiver {
            if event.id() == &open_id {
                crate::settings_app::open_settings();
            } else if event.id() == &box_id {
                // 「桌面整理」 now holds both the app's enable switch and its
                // own options on one page — no separate applications page.
                crate::settings_app::open_settings_at(crate::settings_app::PAGE_BOX);
            } else if event.id() == &restore_id {
                // Same path as the settings button, without the inline confirm:
                // the menu item spells out what it does and the shell restart is
                // visible enough to undo (nothing is lost).
                println!("DCX restoring taskbar by tray request");
                crate::shutdown::restore_taskbar_and_quit();
            } else if event.id() == &quit_id {
                // Signal only. `main` posts WM_QUIT, `Platform::run` returns and
                // the teardown runs in order; `process::exit` here skipped the
                // timer/hook cleanup and could race the app watchdog into
                // launching one more child.
                crate::QUIT.store(true, Ordering::SeqCst);
                println!("DCX quitting by user request");
            }
        }
    });

    // Left click on the icon opens the settings window — the behaviour users
    // expect from a tray app, and the fastest entry when the window was closed.
    // `open_settings` is idempotent (a second call just raises the window).
    std::thread::spawn(|| {
        for event in TrayIconEvent::receiver() {
            let opened = match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => true,
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => true,
                _ => false,
            };
            if opened {
                crate::settings_app::open_settings();
            }
        }
    });

    tray
}

/// Brand mark, embedded as raw RGBA at build time (`tools/gen_brand.py` writes
/// these). Keeping the bytes in the binary means the tray never touches the
/// disk and the icon survives being moved or installed anywhere.
const TRAY_ON_DARK: &[u8] = include_bytes!("../../../brand/embed/tray-32-on-dark.rgba");
const TRAY_ON_LIGHT: &[u8] = include_bytes!("../../../brand/embed/tray-32-on-light.rgba");
const TRAY_PX: u32 = 32;

/// White glyph on a dark taskbar, dark glyph on a light one: the same mark
/// inverted, following `AppsUseLightTheme` like any other shell icon.
fn make_icon() -> Icon {
    let rgba = if dcx_win32::apps_use_light_theme() {
        TRAY_ON_LIGHT
    } else {
        TRAY_ON_DARK
    };
    Icon::from_rgba(rgba.to_vec(), TRAY_PX, TRAY_PX).expect("build tray icon")
}
