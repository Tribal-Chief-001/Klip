# Klip on macOS

Time needed: about 10 minutes.

> **Honest warning.** The macOS program was built and tested only on an automatic test machine (GitHub's), never on a real Mac by the author. Everything below is written from the code and those tests. If a step does not work for you, please [open an issue](https://github.com/Tribal-Chief-001/Klip/issues) and say which step.

**What you need:** a Mac with an **Apple chip (M1, M2, M3, M4…)**. The ready-made program is for Apple Silicon only. On an Intel Mac, [build it yourself](#option-b-build-it-yourself). To check: Apple menu → *About This Mac* → "Chip: Apple M…" is good.

## Step 1: get the program

### Option A: download it

1. Open **Terminal** (press `Cmd+Space`, type *Terminal*, press Enter).
2. Paste, one block at a time:

```bash
mkdir -p ~/.local/bin
cd ~/Downloads
curl -L -O https://github.com/Tribal-Chief-001/Klip/releases/download/v0.10.0-alpha/klip-macos-arm64
curl -L -O https://github.com/Tribal-Chief-001/Klip/releases/download/v0.10.0-alpha/klip-macos-arm64.sha256
shasum -a 256 -c klip-macos-arm64.sha256
```

The last line must print `OK`.

3. Install and allow it:

```bash
install -m755 klip-macos-arm64 ~/.local/bin/klip
xattr -d com.apple.quarantine ~/.local/bin/klip 2>/dev/null
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
```

Close Terminal and open it again so the PATH change is read.

> **Newer version?** Replace `v0.10.0-alpha` with the newest tag from the [Releases page](https://github.com/Tribal-Chief-001/Klip/releases).

**What is the `xattr` line?** Programs downloaded with a browser carry a "quarantine" mark and macOS refuses to run unsigned ones (the message says *"klip cannot be opened because the developer cannot be verified"*). Klip is not signed with an Apple developer certificate (that costs money every year). `curl` does not add the mark, and the `xattr` line removes it if it is there. If you downloaded it with a browser and still see the message: open **System Settings → Privacy & Security**, scroll down to the message about `klip`, and click **Open Anyway**.

### Option B: build it yourself

Needs about 10 minutes. Install Xcode's command-line tools (`xcode-select --install`) and Rust 1.91 or newer from <https://rustup.rs>, then:

```bash
git clone https://github.com/Tribal-Chief-001/Klip.git
cd Klip
cargo build --release -p klip-cli
install -m755 target/release/klip ~/.local/bin/klip
```

## Step 2: start it

```bash
klip start
klip status
```

`klip status` should show **STATE Active**. If macOS asks *"Do you want the application klip to accept incoming network connections?"*, choose **Allow** (Klip connects directly to your other devices).

`klip` alone opens a full-screen app (devices, pairing, history, settings). Press `q` to leave; Klip keeps running.

## Step 3: link your phone or another computer

Go to [Link your devices](05-link-devices.md). Do it once.

## Step 4: make it start by itself

```bash
klip install-service
launchctl load ~/Library/LaunchAgents/com.klip.daemon.plist
```

The first command writes a launch agent (`~/Library/LaunchAgents/com.klip.daemon.plist`) that points at the `klip` you installed; from the next login Klip starts by itself and is restarted if it stops. Do not move or rename `klip` afterwards (run `klip install-service` again if you do). Its output goes to `/tmp/klip.log` and `/tmp/klip.err`.

If you already ran `klip start` by hand, run `klip stop` before `launchctl load`, so there is only one.

**Optional: the menu bar icon** (send an image, pause, open the Klip folder, send a file):

```bash
klip tray install       # starts at every login
klip tray               # start it now
```

The tray does not start Klip for you at login; its menu has **Start Klip** when Klip is stopped. Use `klip install-service` above for that. Details: [the tray icon](../TRAY_AND_TERMINAL_APP.md).

## Step 5: try it

Copy text here, paste on the other device. Send a file:

```bash
klip send-file ~/Pictures/photo.jpg
```

More in [Everyday use](06-daily-use.md).

## Where things are

| What | Where |
|---|---|
| Settings, log, device key | `~/Library/Application Support/com.klip.klip/` (`config.json`, `daemon.log`, `keystore.json`) |
| Files you receive | `~/Downloads/Klip` |
| Daemon's launch agent | `~/Library/LaunchAgents/com.klip.daemon.plist` |
| Tray's launch agent | `~/Library/LaunchAgents/com.klip.tray.plist` |

(The folder under *Application Support* follows the rules of the library Klip uses, and has not been looked at on a real Mac: if it is not there, run `find ~/Library -name daemon.log`.) In Finder: *Go → Go to Folder…* and paste the path.

## Update

Download the new file as in Step 1, then:

```bash
klip stop
install -m755 ~/Downloads/klip-macos-arm64 ~/.local/bin/klip
xattr -d com.apple.quarantine ~/.local/bin/klip 2>/dev/null
klip start
```

Your links are kept.

## Remove Klip completely

```bash
klip tray uninstall
launchctl unload ~/Library/LaunchAgents/com.klip.daemon.plist
rm -f ~/Library/LaunchAgents/com.klip.daemon.plist
klip stop
rm -f ~/.local/bin/klip
```

To also forget this device's identity and links, delete the `com.klip.klip` folder under *Application Support*. Received files in `~/Downloads/Klip` stay. Also remove this Mac on your phone: open Klip, tap the Mac's name, **Remove this device**.

Problems? [Troubleshooting](10-troubleshooting.md) · Questions? [FAQ](09-faq.md)
