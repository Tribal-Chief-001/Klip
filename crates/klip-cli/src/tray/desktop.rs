//! The tray on macOS and Windows: `tao`'s event loop on the main thread and `tray-icon` for the icon and
//! the menu.
//!
//! Everything the tray does comes from `controller` (commands go in, pictures come out); this file only
//! shows the picture and turns clicks into commands. The menu is rebuilt for every new picture.
use super::controller::{Cmd, Out};
use super::model::{menu_plan, Action, Entry, Summary};
use super::raster;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tray_icon::menu::{
    CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

enum UserEvent {
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    Out(Out),
    /// Sent to ourselves once the icon exists: it makes the system draw it right away.
    Wake,
    /// The controller ended (a stop signal, Quit): nothing is left to show, so the loop ends too.
    Exit,
}

/// macOS: a template image (black with alpha) that the system tints for a light or dark menu bar,
/// drawn for a retina menu bar.
#[cfg(target_os = "macos")]
const ICON_SIZE: u32 = 44;
#[cfg(target_os = "macos")]
fn icon_ink() -> [u8; 3] {
    [0, 0, 0]
}

/// Windows: a plain icon, light on a dark taskbar and dark on a light one (read from the registry).
#[cfg(windows)]
const ICON_SIZE: u32 = 32;
#[cfg(windows)]
fn icon_ink() -> [u8; 3] {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "/v",
            "SystemUsesLightTheme",
        ])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .ok();
    let light = out
        .and_then(|o| raster::parse_reg_dword(&String::from_utf8_lossy(&o.stdout)))
        .map(|v| v != 0);
    raster::ink_for_taskbar(light)
}

fn icon_for(summary: &Summary) -> Result<Icon, String> {
    let img = raster::render(summary.state, summary.badge, ICON_SIZE, icon_ink());
    Icon::from_rgba(img.data, img.w, img.h).map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn with_icon(builder: TrayIconBuilder, icon: Icon) -> TrayIconBuilder {
    builder.with_icon_templated(icon)
}

#[cfg(windows)]
fn with_icon(builder: TrayIconBuilder, icon: Icon) -> TrayIconBuilder {
    builder.with_icon(icon)
}

#[cfg(target_os = "macos")]
fn set_icon(tray: &TrayIcon, icon: Icon) {
    let _ = tray.set_icon_templated(Some(icon));
}

#[cfg(windows)]
fn set_icon(tray: &TrayIcon, icon: Icon) {
    let _ = tray.set_icon(Some(icon));
}

/// `&` would become an access key in a menu label.
fn escape(text: &str) -> String {
    text.replace('&', "&&")
}

fn items(entries: &[Entry]) -> Result<Vec<Box<dyn IsMenuItem>>, String> {
    let mut out: Vec<Box<dyn IsMenuItem>> = Vec::new();
    for entry in entries {
        let item: Box<dyn IsMenuItem> = match entry {
            Entry::Separator => Box::new(PredefinedMenuItem::separator()),
            Entry::Label(text) => Box::new(MenuItem::with_id("label", escape(text), false, None)),
            Entry::Item {
                label,
                enabled,
                action,
            } => Box::new(MenuItem::with_id(
                action.id(),
                escape(label),
                *enabled,
                None,
            )),
            Entry::Check {
                label,
                checked,
                action,
            } => Box::new(CheckMenuItem::with_id(
                action.id(),
                escape(label),
                true,
                *checked,
                None,
            )),
            Entry::Submenu { label, items: sub } => {
                let inner = items(sub)?;
                let refs: Vec<&dyn IsMenuItem> = inner.iter().map(|i| i.as_ref()).collect();
                Box::new(
                    Submenu::with_items(escape(label), true, &refs).map_err(|e| e.to_string())?,
                )
            }
        };
        out.push(item);
    }
    Ok(out)
}

fn menu_for(summary: &Summary) -> Result<Menu, String> {
    let all = items(&menu_plan(summary))?;
    let refs: Vec<&dyn IsMenuItem> = all.iter().map(|i| i.as_ref()).collect();
    Menu::with_items(&refs).map_err(|e| e.to_string())
}

fn tooltip(summary: &Summary) -> String {
    let body = summary.tooltip_body();
    let text = if body.is_empty() {
        summary.title.clone()
    } else {
        format!("{}\n{body}", summary.title)
    };
    text.chars().take(120).collect()
}

fn apply(tray: &TrayIcon, summary: &Summary) {
    match (icon_for(summary), menu_for(summary)) {
        (Ok(icon), Ok(menu)) => {
            set_icon(tray, icon);
            tray.set_menu(Some(Box::new(menu)));
            let _ = tray.set_tooltip(Some(tooltip(summary)));
        }
        (Err(e), _) | (_, Err(e)) => eprintln!("cannot update the tray icon: {e}"),
    }
}

fn build(summary: &Summary) -> Result<TrayIcon, String> {
    with_icon(TrayIconBuilder::new(), icon_for(summary)?)
        .with_menu(Box::new(menu_for(summary)?))
        .with_tooltip(tooltip(summary))
        .with_menu_on_left_click(true)
        .build()
        .map_err(|e| e.to_string())
}

/// Show the tray until it is quit. Must run on the main thread; it does not return (the process ends
/// when the event loop exits), only an early failure comes back as an error.
pub fn run(
    commands: UnboundedSender<Cmd>,
    mut pictures: UnboundedReceiver<Out>,
) -> Result<(), String> {
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    // A menu-bar tray has no Dock icon and no application menu.
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = event_loop.create_proxy();
    {
        let proxy = proxy.clone();
        TrayIconEvent::set_event_handler(Some(move |e| {
            let _ = proxy.send_event(UserEvent::Tray(e));
        }));
    }
    {
        let proxy = proxy.clone();
        MenuEvent::set_event_handler(Some(move |e| {
            let _ = proxy.send_event(UserEvent::Menu(e));
        }));
    }
    {
        let proxy = proxy.clone();
        tokio::spawn(async move {
            while let Some(out) = pictures.recv().await {
                if proxy.send_event(UserEvent::Out(out)).is_err() {
                    return;
                }
            }
            // The controller is gone. Without this the loop would run on, a dead icon that ignores
            // SIGTERM and Ctrl-C (tokio's handler is what swallowed them).
            let _ = proxy.send_event(UserEvent::Exit);
        });
    }

    let mut tray: Option<TrayIcon> = None;
    let mut summary = Summary::stopped();
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            // The icon is made once the loop runs (before that macOS may ignore it).
            Event::NewEvents(StartCause::Init) => match build(&summary) {
                Ok(icon) => {
                    tray = Some(icon);
                    let _ = proxy.send_event(UserEvent::Wake);
                }
                Err(e) => {
                    eprintln!("cannot show the tray icon: {e}");
                    std::process::exit(1);
                }
            },
            Event::UserEvent(UserEvent::Out(Out::Summary(s))) => {
                summary = s;
                if let Some(icon) = &tray {
                    apply(icon, &summary);
                }
            }
            // The problem is already the first line of the menu.
            Event::UserEvent(UserEvent::Out(Out::Problem(_)))
            | Event::UserEvent(UserEvent::Wake) => {}
            Event::UserEvent(UserEvent::Exit) => *flow = ControlFlow::Exit,
            Event::UserEvent(UserEvent::Menu(e)) => match Action::from_id(&e.id.0) {
                Some(Action::Quit) => *flow = ControlFlow::Exit,
                Some(action) => {
                    let _ = commands.send(Cmd::Do(action));
                }
                None => {} // a label or a stale id
            },
            Event::UserEvent(UserEvent::Tray(TrayIconEvent::Click {
                button,
                button_state,
                ..
            })) => match (button, button_state) {
                (MouseButton::Middle, MouseButtonState::Up) => {
                    let _ = commands.send(Cmd::Do(Action::SendImage));
                }
                // The menu is opening: ask for a fresh picture.
                (MouseButton::Left | MouseButton::Right, MouseButtonState::Down) => {
                    let _ = commands.send(Cmd::Refresh);
                }
                _ => {}
            },
            _ => {}
        }
    })
}
