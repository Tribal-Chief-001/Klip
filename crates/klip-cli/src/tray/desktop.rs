//! The tray on macOS: `tao`'s event loop on the main thread and `tray-icon` for the icon and the menu.
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
    /// Sent to ourselves once the icon exists: it makes macOS draw it right away.
    Wake,
}

/// The icon is a template image: black with alpha, which the system tints for a light or dark menu bar.
const ICON_SIZE: u32 = 44;
const ICON_INK: [u8; 3] = [0, 0, 0];

fn icon_for(summary: &Summary) -> Result<Icon, String> {
    let img = raster::render(summary.state, summary.badge, ICON_SIZE, ICON_INK);
    Icon::from_rgba(img.data, img.w, img.h).map_err(|e| e.to_string())
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
            let _ = tray.set_icon_templated(Some(icon));
            tray.set_menu(Some(Box::new(menu)));
            let _ = tray.set_tooltip(Some(tooltip(summary)));
        }
        (Err(e), _) | (_, Err(e)) => eprintln!("cannot update the tray icon: {e}"),
    }
}

fn build(summary: &Summary) -> Result<TrayIcon, String> {
    TrayIconBuilder::new()
        .with_icon_templated(icon_for(summary)?)
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
    use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};

    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    // A menu-bar tray has no Dock icon and no application menu.
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
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
                    break;
                }
            }
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
