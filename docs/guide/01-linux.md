# Klip on Linux

Time needed: about 5 minutes. Tested on a real machine (Linux Mint, X11).

**What you need:** a 64-bit Intel/AMD computer, a desktop session (X11, see the Wayland note below), internet for the first link. The ready-made program was built on Linux Mint 22 / Ubuntu 24.04 (glibc 2.39). On an older distribution, [build it from source](#option-b-build-it-yourself) instead.

## Step 1: get the program

### Option A: download the ready-made program (easiest)

Open a terminal and paste these lines one block at a time.

```bash
mkdir -p ~/.local/bin
cd ~/Downloads
curl -L -O https://github.com/Tribal-Chief-001/Klip/releases/download/v0.10.0-alpha/klip-linux-x86_64
curl -L -O https://github.com/Tribal-Chief-001/Klip/releases/download/v0.10.0-alpha/klip-linux-x86_64.sha256
```

Check that the download is not damaged (it must print `OK`):

```bash
sha256sum -c klip-linux-x86_64.sha256
```

Install it:

```bash
install -Dm755 klip-linux-x86_64 ~/.local/bin/klip
```

> **Newer version?** `v0.10.0-alpha` is the version these lines name. Look at the [Releases page](https://github.com/Tribal-Chief-001/Klip/releases) and replace it with the newest tag. (The `/latest/` link does not work yet, because every release is still marked "pre-release".)

Make sure `~/.local/bin` is in your PATH: type `klip` — if the terminal says "command not found", run

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
```

close the terminal, open a new one, and try again.

### Option B: build it yourself

Needs about 5 minutes and 2 GB of disk. Install Rust (1.91 or newer) from <https://rustup.rs>, then:

```bash
git clone https://github.com/Tribal-Chief-001/Klip.git
cd Klip
cargo build --release -p klip-cli
install -Dm755 target/release/klip ~/.local/bin/klip
```

## Step 2: start it

```bash
klip start
klip status
```

`klip status` should show **STATE Active**. The box also shows your **DEVICE** name with a fingerprint like `J73D-FACE-MFHA` — that is how your devices recognise each other.

Typing `klip` alone opens a full-screen app (devices, pairing, history, settings). Press `q` to leave it; Klip keeps running.

## Step 3: link your phone or another computer

Go to [Link your devices](05-link-devices.md). Do it once.

## Step 4: make it start by itself

Without this, Klip stops when you restart the computer.

```bash
klip install-service
systemctl --user enable --now klip.service
```

Check: `systemctl --user status klip.service` says *active (running)*. If the program ever crashes, systemd starts it again in about 3 seconds.

**Optional: the tray icon** (a small menu in your panel to send an image, pause, open the folder, send a file):

```bash
klip tray install       # starts at every login
klip tray               # start it now
```

Details: [the tray icon](../TRAY_AND_TERMINAL_APP.md).

**Optional: "Send a file…" in the tray menu** needs `zenity` (GNOME/Cinnamon/XFCE) or `kdialog` (KDE) installed; most desktops have one.

## Step 5: try it

Copy some text here, paste it on the other device. Or send a file:

```bash
klip send-file ~/Pictures/photo.jpg
```

More in [Everyday use](06-daily-use.md).

## Where things are

| What | Where |
|---|---|
| Settings | `~/.config/klip/config.json` ([all settings](07-settings.md)) |
| Log (read this when something is wrong) | `~/.config/klip/daemon.log` |
| Your device key (secret, never share it) | `~/.config/klip/keystore.json` |
| Files you receive | `~/Downloads/Klip` |
| The tray's start-at-login entry | `~/.config/autostart/klip-tray.desktop` |
| The daemon's service file | `~/.config/systemd/user/klip.service` |

## Wayland (important)

Klip reads and writes the **X11** clipboard. On a Wayland desktop it works only for programs running through XWayland, and GNOME/Wayland needs more work that is not finished. If nothing syncs, log in with the **X11 / "Cinnamon" / "Xorg"** session (choose it at the login screen) and try again. Linux Mint Cinnamon uses X11 by default.

## Update

Download the new `klip-linux-x86_64` as in Step 1, then:

```bash
klip stop
install -Dm755 ~/Downloads/klip-linux-x86_64 ~/.local/bin/klip
klip start
```

If you use the service, `systemctl --user restart klip.service` instead of stop/start. Your links are kept.

## Remove Klip completely

```bash
klip tray uninstall
systemctl --user disable --now klip.service
rm -f ~/.config/systemd/user/klip.service
klip stop
rm -f ~/.local/bin/klip
```

To also forget this device's identity and all links, delete `~/.config/klip` (you would have to link everything again). Received files in `~/Downloads/Klip` are yours; Klip never deletes them. Also remove this computer from your phone: open Klip, tap the computer's name, then **Remove this device**.

Problems? [Troubleshooting](10-troubleshooting.md) · Questions? [FAQ](09-faq.md)
