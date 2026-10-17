//! What the tray knows and says: no D-Bus and no I/O, so every state can be tested.
use klip_proto::ipc::{DaemonStatusInfo, ImageStateInfo};
use klip_tui::fmt::{ago, bytes, remaining};

/// Pause choices of the menu (seconds); the last one means "until I resume".
pub const PAUSE_CHOICES: [(&str, u64); 4] = [
    ("10 minutes", 600),
    ("1 hour", 3600),
    ("8 hours", 28_800),
    ("Until I resume", 315_360_000),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Stopped,
    NoDevices,
    Waiting,
    Connected,
    Paused,
}

impl State {
    /// The word in the icon's name.
    pub fn key(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::NoDevices => "no_devices",
            Self::Waiting => "waiting",
            Self::Connected => "connected",
            Self::Paused => "paused",
        }
    }
}

/// One consistent picture of the daemon. Ages are left out on purpose: a picture that changes every
/// second would be announced on D-Bus every poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub state: State,
    pub title: String,
    pub subtitle: String,
    /// A copied image is waiting and sending is manual: the icon gets a corner dot.
    pub badge: bool,
    pub any_online: bool,
    /// Size of the held image.
    pub image: Option<usize>,
    pub auto_images: bool,
    /// The daemon answered the image question (an older one does not).
    pub images_known: bool,
    /// The last action that failed, shown as the first line of the menu for a while (set by the
    /// controller; the platforms without desktop notifications rely on it).
    pub problem: Option<String>,
    /// While files move: "Sending 2 files, 45%" (never a file name). Set by the controller from `GetTransfers`.
    pub files: Option<String>,
    /// A file picker exists on this system and the daemon knows files: the menu offers "Send a file...".
    pub can_send_file: bool,
}

impl Summary {
    pub fn stopped() -> Self {
        Self {
            state: State::Stopped,
            title: "Klip is stopped".into(),
            subtitle: "Start it to sync your clipboard".into(),
            badge: false,
            any_online: false,
            image: None,
            auto_images: false,
            images_known: false,
            problem: None,
            files: None,
            can_send_file: false,
        }
    }

    /// The hover text: the title, the detail and the image line.
    pub fn tooltip_body(&self) -> String {
        let mut lines = Vec::new();
        if !self.subtitle.is_empty() {
            lines.push(self.subtitle.clone());
        }
        if let Some(n) = self.image {
            lines.push(format!("Image ready to send ({})", bytes(n)));
        }
        lines.join("\n")
    }
}

/// An engine or network message in words a person can act on.
pub fn plain_error(raw: &str) -> String {
    let r = raw.to_lowercase();
    if raw.is_empty() {
        String::new()
    } else if r.contains("unpaired by the other device") {
        "The other device removed this device. Remove it here too, then link again.".into()
    } else if r.contains("paused") {
        "That device has syncing paused.".into()
    } else if r.contains("clipboard") && r.contains("write") {
        "That device couldn't write to its clipboard.".into()
    } else if r.contains("no addressing information")
        || r.contains("cannot reach")
        || r.contains("unreachable")
    {
        "Can't find it right now. It reconnects by itself when it's back.".into()
    } else if r.contains("timed out") {
        "It didn't answer in time.".into()
    } else {
        raw.chars().take(90).collect()
    }
}

fn names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [a, b] => format!("{a} and {b}"),
        [a, rest @ ..] => format!("{a} and {} more", rest.len()),
    }
}

/// The picture from the daemon's answers; `status` is `None` when the daemon did not answer.
pub fn summarize(status: Option<&DaemonStatusInfo>, images: Option<&ImageStateInfo>) -> Summary {
    let Some(s) = status else {
        return Summary::stopped();
    };
    let online: Vec<&str> = s
        .peers
        .iter()
        .filter(|p| p.is_online)
        .map(|p| p.name.as_str())
        .collect();
    let all: Vec<&str> = s.peers.iter().map(|p| p.name.as_str()).collect();
    let (state, title, subtitle) = if s.is_paused {
        let left = s.paused_remaining_secs;
        let subtitle = if left > 365 * 24 * 3600 {
            remaining(left)
        } else if left > 0 {
            format!("{} left", remaining(left))
        } else {
            String::new()
        };
        (State::Paused, "Paused".to_string(), subtitle)
    } else if s.peers.is_empty() {
        (
            State::NoDevices,
            "No devices linked".to_string(),
            "Add your phone or another computer".to_string(),
        )
    } else if !online.is_empty() {
        let subtitle = match s.last_sync_time_secs_ago {
            Some(secs) => format!("Last sync {}", ago(secs)),
            None => "Nothing synced yet".to_string(),
        };
        (
            State::Connected,
            format!("Connected to {}", names(&online)),
            subtitle,
        )
    } else {
        let why = s
            .peers
            .iter()
            .filter_map(|p| p.last_error.as_deref())
            .map(plain_error)
            .find(|e| !e.is_empty())
            .unwrap_or_else(|| "Not connected yet. It reconnects by itself.".to_string());
        (State::Waiting, format!("Waiting for {}", names(&all)), why)
    };
    let image = images
        .and_then(|i| i.waiting.as_ref())
        .map(|w| w.size_bytes);
    let auto_images = images.is_some_and(|i| i.auto);
    Summary {
        state,
        title,
        subtitle,
        badge: image.is_some() && !auto_images,
        any_online: !online.is_empty(),
        image,
        auto_images,
        images_known: images.is_some(),
        problem: None,
        files: None,
        can_send_file: false,
    }
}

/// "Sending 2 files, 45%" / "Receiving 1 file, 10%" while files move; `None` when nothing does. Never a name.
pub fn transfer_line(rows: &[klip_proto::ipc::TransferInfo]) -> Option<String> {
    use klip_proto::ipc::{TransferDirection, TransferState};
    let running: Vec<_> = rows
        .iter()
        .filter(|t| matches!(t.state, TransferState::Waiting | TransferState::Active))
        .collect();
    if running.is_empty() {
        return None;
    }
    let sending = running
        .iter()
        .filter(|t| t.direction == TransferDirection::Sending)
        .count();
    let receiving = running.len() - sending;
    let total: u64 = running.iter().map(|t| t.size).sum();
    let done: u64 = running.iter().map(|t| t.done).sum();
    let percent = (done * 100).checked_div(total).unwrap_or(100);
    let part = |verb: &str, n: usize| format!("{verb} {n} file{}", if n == 1 { "" } else { "s" });
    let what = match (sending, receiving) {
        (s, 0) => part("Sending", s),
        (0, r) => part("Receiving", r),
        (s, r) => format!("{}, {}", part("Sending", s), part("receiving", r)),
    };
    Some(format!("{what}, {percent}%"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Start,
    SendImage,
    SetAuto(bool),
    Pause(u64),
    Resume,
    OpenTerminal,
    /// Open the folder received files land in.
    OpenFolder,
    /// Pick files in a dialog and send them (Linux, when a picker is installed).
    SendFile,
    Quit,
}

impl Action {
    /// The menu id a platform stores on an item and gives back on a click.
    pub fn id(&self) -> String {
        match self {
            Self::Start => "start".into(),
            Self::SendImage => "send".into(),
            Self::SetAuto(on) => format!("auto:{}", u8::from(*on)),
            Self::Pause(secs) => format!("pause:{secs}"),
            Self::Resume => "resume".into(),
            Self::OpenTerminal => "terminal".into(),
            Self::OpenFolder => "folder".into(),
            Self::SendFile => "sendfile".into(),
            Self::Quit => "quit".into(),
        }
    }

    /// The action behind a menu id; anything else (a stale or foreign id) is `None`.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "start" => Some(Self::Start),
            "send" => Some(Self::SendImage),
            "auto:1" => Some(Self::SetAuto(true)),
            "auto:0" => Some(Self::SetAuto(false)),
            "resume" => Some(Self::Resume),
            "terminal" => Some(Self::OpenTerminal),
            "folder" => Some(Self::OpenFolder),
            "sendfile" => Some(Self::SendFile),
            "quit" => Some(Self::Quit),
            _ => id
                .strip_prefix("pause:")
                .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|n| n.parse().ok())
                .map(Self::Pause),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Label(String),
    Separator,
    Item {
        label: String,
        enabled: bool,
        action: Action,
    },
    /// A tick box; `action` is what ticking (or unticking) it does.
    Check {
        label: String,
        checked: bool,
        action: Action,
    },
    Submenu {
        label: String,
        items: Vec<Entry>,
    },
}

fn item(label: &str, action: Action) -> Entry {
    Entry::Item {
        label: label.to_string(),
        enabled: true,
        action,
    }
}

/// The menu for a picture. Left and right click show the same one.
pub fn menu_plan(s: &Summary) -> Vec<Entry> {
    let stopped = s.state == State::Stopped;
    let paused = s.state == State::Paused;
    let mut menu = Vec::new();
    if let Some(problem) = &s.problem {
        menu.push(Entry::Label(format!("\u{26a0} {problem}")));
    }
    menu.push(Entry::Label(s.title.clone()));
    if let Some(files) = &s.files {
        menu.push(Entry::Label(files.clone()));
    }
    menu.push(Entry::Separator);
    if stopped {
        menu.push(item("Start Klip", Action::Start));
    } else {
        if s.images_known {
            let label = match s.image {
                Some(n) => format!("Send the last copied image ({})", bytes(n)),
                None => "Send the last copied image".to_string(),
            };
            menu.push(Entry::Item {
                label,
                enabled: s.image.is_some() && s.any_online && !paused,
                action: Action::SendImage,
            });
            menu.push(Entry::Check {
                label: "Send images automatically".into(),
                checked: s.auto_images,
                action: Action::SetAuto(!s.auto_images),
            });
        }
        if s.can_send_file {
            menu.push(Entry::Item {
                label: "Send a file...".into(),
                enabled: s.any_online && !paused,
                action: Action::SendFile,
            });
        }
        if paused {
            menu.push(item("Resume syncing", Action::Resume));
        } else {
            menu.push(Entry::Submenu {
                label: "Pause syncing".into(),
                items: PAUSE_CHOICES
                    .iter()
                    .map(|(text, secs)| item(text, Action::Pause(*secs)))
                    .collect(),
            });
        }
    }
    menu.push(Entry::Separator);
    menu.push(item("Open the Klip folder", Action::OpenFolder));
    menu.push(item("Open the Klip terminal app", Action::OpenTerminal));
    menu.push(item("Quit the tray", Action::Quit));
    menu
}

#[cfg(test)]
mod tests {
    use super::*;
    use klip_proto::hlc::NodeId;
    use klip_proto::ipc::{PeerStatusInfo, WaitingImageInfo};

    fn peer(name: &str, online: bool, error: Option<&str>) -> PeerStatusInfo {
        PeerStatusInfo {
            node_id: NodeId([7; 32]),
            name: name.into(),
            is_online: online,
            last_error: error.map(str::to_string),
        }
    }

    fn status(peers: Vec<PeerStatusInfo>) -> DaemonStatusInfo {
        DaemonStatusInfo {
            is_active: true,
            is_paused: false,
            paused_remaining_secs: 0,
            platform: "Linux (X11)".into(),
            memory_rss_bytes: 1000,
            node_id: NodeId([1; 32]),
            relay_mode: "n0".into(),
            relay_state: "relay connected".into(),
            pairing_open: false,
            peers,
            total_synced_clips: 0,
            last_sync_time_secs_ago: None,
            last_clip_preview: None,
            last_clip_size_bytes: 0,
        }
    }

    fn images(auto: bool, waiting: bool) -> ImageStateInfo {
        ImageStateInfo {
            auto,
            waiting: waiting.then(|| WaitingImageInfo {
                size_bytes: 186_183,
                width: 1920,
                height: 1080,
                age_secs: 130,
                kind: "png".into(),
            }),
        }
    }

    #[test]
    fn a_daemon_that_does_not_answer_is_stopped() {
        let s = summarize(None, None);
        assert_eq!(s.state, State::Stopped);
        assert_eq!(s.title, "Klip is stopped");
        assert!(!s.badge && !s.images_known);
    }

    #[test]
    fn no_devices_connected_waiting_and_paused_each_say_what_is_going_on() {
        let none = summarize(Some(&status(vec![])), None);
        assert_eq!(
            (none.state, none.title.as_str()),
            (State::NoDevices, "No devices linked")
        );

        let mut st = status(vec![peer("Vivo", true, None), peer("Pad", false, None)]);
        st.last_sync_time_secs_ago = Some(125);
        let on = summarize(Some(&st), None);
        assert_eq!(on.state, State::Connected);
        assert_eq!(on.title, "Connected to Vivo");
        assert_eq!(on.subtitle, "Last sync 2 min ago");
        st.last_sync_time_secs_ago = None;
        assert_eq!(summarize(Some(&st), None).subtitle, "Nothing synced yet");

        let waiting = summarize(
            Some(&status(vec![peer(
                "Vivo",
                false,
                Some("cannot reach peer: timed out"),
            )])),
            None,
        );
        assert_eq!(waiting.state, State::Waiting);
        assert_eq!(waiting.title, "Waiting for Vivo");
        assert!(waiting.subtitle.contains("reconnects by itself"));

        let mut paused = status(vec![peer("Vivo", true, None)]);
        paused.is_paused = true;
        paused.paused_remaining_secs = 300;
        let p = summarize(Some(&paused), None);
        assert_eq!((p.state, p.subtitle.as_str()), (State::Paused, "5:00 left"));
        paused.paused_remaining_secs = 315_360_000;
        assert_eq!(summarize(Some(&paused), None).subtitle, "until you resume");
    }

    #[test]
    fn several_device_names_are_shortened() {
        assert_eq!(names(&["A"]), "A");
        assert_eq!(names(&["A", "B"]), "A and B");
        assert_eq!(names(&["A", "B", "C", "D"]), "A and 3 more");
    }

    #[test]
    fn a_waiting_image_adds_the_badge_only_in_manual_mode() {
        let st = status(vec![peer("Vivo", true, None)]);
        let manual = summarize(Some(&st), Some(&images(false, true)));
        assert!(manual.badge);
        assert_eq!(manual.image, Some(186_183));
        assert!(manual
            .tooltip_body()
            .contains("Image ready to send (181.8 KiB)"));
        assert!(!summarize(Some(&st), Some(&images(true, true))).badge);
        assert!(!summarize(Some(&st), Some(&images(false, false))).badge);
        assert!(
            !summarize(Some(&st), None).images_known,
            "an older daemon: no image items"
        );
    }

    #[test]
    fn plain_errors_say_what_a_person_can_do() {
        assert!(
            plain_error("unpaired by the other device; pair again").contains("removed this device")
        );
        assert!(plain_error("the other device has sync paused").contains("paused"));
        assert!(
            plain_error("the other device could not write its clipboard").contains("clipboard")
        );
        assert!(plain_error("cannot reach peer: timed out").contains("reconnects by itself"));
        assert_eq!(plain_error(&"x".repeat(200)), "x".repeat(90));
        assert_eq!(plain_error(""), "");
    }

    fn labels(plan: &[Entry]) -> Vec<String> {
        plan.iter()
            .map(|e| match e {
                Entry::Label(t) => t.clone(),
                Entry::Separator => "-".into(),
                Entry::Item { label, enabled, .. } => {
                    format!("{label}{}", if *enabled { "" } else { " (off)" })
                }
                Entry::Check { label, checked, .. } => {
                    format!("[{}] {label}", if *checked { "x" } else { " " })
                }
                Entry::Submenu { label, .. } => format!("{label} >"),
            })
            .collect()
    }

    #[test]
    fn the_menu_of_a_stopped_daemon_offers_start() {
        let plan = menu_plan(&Summary::stopped());
        assert_eq!(
            labels(&plan),
            [
                "Klip is stopped",
                "-",
                "Start Klip",
                "-",
                "Open the Klip folder",
                "Open the Klip terminal app",
                "Quit the tray"
            ]
        );
        assert!(plan.contains(&item("Start Klip", Action::Start)));
    }

    #[test]
    fn the_menu_of_a_connected_daemon_with_a_waiting_image() {
        let st = status(vec![peer("Vivo", true, None)]);
        let plan = menu_plan(&summarize(Some(&st), Some(&images(false, true))));
        assert_eq!(
            labels(&plan),
            [
                "Connected to Vivo",
                "-",
                "Send the last copied image (181.8 KiB)",
                "[ ] Send images automatically",
                "Pause syncing >",
                "-",
                "Open the Klip folder",
                "Open the Klip terminal app",
                "Quit the tray"
            ]
        );
        assert!(plan.iter().any(|e| matches!(
            e,
            Entry::Check {
                action: Action::SetAuto(true),
                ..
            }
        )));
        let Entry::Submenu { items, .. } = &plan[4] else {
            panic!()
        };
        assert_eq!(items.len(), 4);
        assert!(items.contains(&item("1 hour", Action::Pause(3600))));
    }

    #[test]
    fn sending_is_off_without_an_image_a_connected_device_or_while_paused() {
        let offline = status(vec![peer("Vivo", false, None)]);
        let plan = menu_plan(&summarize(Some(&offline), Some(&images(false, true))));
        assert!(labels(&plan)
            .iter()
            .any(|l| l == "Send the last copied image (181.8 KiB) (off)"));
        let online = status(vec![peer("Vivo", true, None)]);
        let plan = menu_plan(&summarize(Some(&online), Some(&images(false, false))));
        assert!(labels(&plan)
            .iter()
            .any(|l| l == "Send the last copied image (off)"));
        let mut paused = online;
        paused.is_paused = true;
        paused.paused_remaining_secs = 60;
        let plan = menu_plan(&summarize(Some(&paused), Some(&images(false, true))));
        let l = labels(&plan);
        assert!(l
            .iter()
            .any(|x| x.ends_with("(off)") && x.starts_with("Send the last")));
        assert!(l.contains(&"Resume syncing".to_string()));
        assert!(!l.contains(&"Pause syncing >".to_string()));
    }

    #[test]
    fn an_older_daemon_without_images_has_no_image_items() {
        let st = status(vec![peer("Vivo", true, None)]);
        let l = labels(&menu_plan(&summarize(Some(&st), None)));
        assert!(!l.iter().any(|x| x.contains("image")));
        assert!(l.contains(&"Pause syncing >".to_string()));
    }

    #[test]
    fn every_action_survives_a_round_trip_through_its_menu_id() {
        let all = [
            Action::Start,
            Action::SendImage,
            Action::SetAuto(true),
            Action::SetAuto(false),
            Action::Pause(600),
            Action::Pause(315_360_000),
            Action::Resume,
            Action::OpenTerminal,
            Action::OpenFolder,
            Action::SendFile,
            Action::Quit,
        ];
        for a in all {
            assert_eq!(Action::from_id(&a.id()), Some(a), "{a:?} via {:?}", a.id());
        }
        let ids: std::collections::HashSet<String> = all.iter().map(|a| a.id()).collect();
        assert_eq!(ids.len(), all.len(), "ids are unique");
    }

    #[test]
    fn a_menu_id_that_is_not_ours_is_ignored() {
        for junk in [
            "", "pause:", "pause:x", "pause:-1", "auto:2", "auto", "send ", "rm -rf /", "quit\n",
        ] {
            assert_eq!(Action::from_id(junk), None, "{junk:?}");
        }
    }

    fn row(
        outgoing: bool,
        size: u64,
        done: u64,
        state: klip_proto::ipc::TransferState,
    ) -> klip_proto::ipc::TransferInfo {
        use klip_proto::ipc::TransferDirection;
        klip_proto::ipc::TransferInfo {
            id: [1; 16],
            peer: "Pixel".into(),
            direction: if outgoing {
                TransferDirection::Sending
            } else {
                TransferDirection::Receiving
            },
            name: "private-name.pdf".into(),
            size,
            done,
            state,
            error: None,
            saved_to: None,
            started_ms: 0,
            finished_ms: None,
        }
    }

    #[test]
    fn the_file_line_counts_running_transfers_and_never_shows_a_name() {
        use klip_proto::ipc::TransferState::{Active, Done, Failed, Waiting};
        assert_eq!(transfer_line(&[]), None);
        assert_eq!(
            transfer_line(&[row(true, 100, 100, Done), row(false, 5, 0, Failed)]),
            None
        );
        assert_eq!(
            transfer_line(&[row(true, 100, 45, Active)]),
            Some("Sending 1 file, 45%".into())
        );
        assert_eq!(
            transfer_line(&[row(false, 100, 10, Active), row(false, 100, 30, Waiting)]),
            Some("Receiving 2 files, 20%".into())
        );
        assert_eq!(
            transfer_line(&[row(true, 100, 50, Active), row(false, 100, 10, Active)]),
            Some("Sending 1 file, receiving 1 file, 30%".into())
        );
        assert_eq!(
            transfer_line(&[row(true, 0, 0, Active)]),
            Some("Sending 1 file, 100%".into())
        );
        assert!(!transfer_line(&[row(true, 100, 45, Active)])
            .unwrap()
            .contains("private"));
    }

    #[test]
    fn the_menu_offers_the_folder_always_and_sending_a_file_only_where_a_picker_exists_and_a_device_is_online(
    ) {
        let st = status(vec![peer("Vivo", true, None)]);
        let base = summarize(Some(&st), None);
        let has = |s: &Summary, a: Action| {
            menu_plan(s)
                .iter()
                .any(|e| matches!(e, Entry::Item { action, enabled: true, .. } if *action == a))
        };
        assert!(has(&base, Action::OpenFolder));
        assert!(!has(&base, Action::SendFile), "no picker on this system");
        let mut with_picker = base.clone();
        with_picker.can_send_file = true;
        assert!(has(&with_picker, Action::SendFile));
        let offline = summarize(Some(&status(vec![peer("Vivo", false, None)])), None);
        let mut offline_picker = offline;
        offline_picker.can_send_file = true;
        assert!(!has(&offline_picker, Action::SendFile), "nobody to send to");
        assert!(
            has(&Summary::stopped(), Action::OpenFolder),
            "the folder opens even when Klip is stopped"
        );
    }

    #[test]
    fn a_file_line_is_shown_under_the_title_while_files_move() {
        let mut s = Summary::stopped();
        s.files = Some("Sending 1 file, 45%".into());
        let plan = menu_plan(&s);
        assert_eq!(plan[1], Entry::Label("Sending 1 file, 45%".into()));
    }

    #[test]
    fn a_problem_is_the_first_line_of_the_menu() {
        let mut s = Summary::stopped();
        s.problem = Some("boom".into());
        let plan = menu_plan(&s);
        assert_eq!(plan[0], Entry::Label("\u{26a0} boom".into()));
        assert_eq!(plan[1], Entry::Label(s.title.clone()));
        s.problem = None;
        assert_eq!(menu_plan(&s)[0], Entry::Label(s.title.clone()));
    }

    #[test]
    fn the_switch_offers_the_opposite_of_what_it_shows() {
        let st = status(vec![peer("Vivo", true, None)]);
        let on = menu_plan(&summarize(Some(&st), Some(&images(true, false))));
        assert!(on.iter().any(|e| matches!(
            e,
            Entry::Check {
                checked: true,
                action: Action::SetAuto(false),
                ..
            }
        )));
    }
}
