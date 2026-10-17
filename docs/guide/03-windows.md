# Klip on Windows

Time needed: about 10 minutes.

> **Honest warning.** The Windows program was built and tested only on an automatic test machine (GitHub's), never on a real PC by the author. It starts a daemon, talks to it, and passes its tests there. Everything below is written from the code and those tests. If a step does not work for you, please [open an issue](https://github.com/Tribal-Chief-001/Klip/issues) and say which step.

**What you need:** Windows 10 or 11, 64-bit. Nothing else to install.

## Step 1: get the program

1. Open **PowerShell** (press the Windows key, type *PowerShell*, press Enter).
2. Paste, one block at a time:

```powershell
mkdir $env:USERPROFILE\Klip -Force
cd $env:USERPROFILE\Klip
Invoke-WebRequest https://github.com/Tribal-Chief-001/Klip/releases/download/v0.10.0-alpha/klip-windows-x86_64.exe -OutFile klip.exe
Invoke-WebRequest https://github.com/Tribal-Chief-001/Klip/releases/download/v0.10.0-alpha/klip-windows-x86_64.exe.sha256 -OutFile klip.sha256
```

3. Check the download is not damaged. These two lines must show the **same** text (letters may be upper or lower case):

```powershell
(Get-FileHash klip.exe -Algorithm SHA256).Hash
(Get-Content klip.sha256).Split(" ")[0]
```

4. Make `klip` work from any PowerShell window (one time):

```powershell
$p = [Environment]::GetEnvironmentVariable("Path","User")
[Environment]::SetEnvironmentVariable("Path", "$p;$env:USERPROFILE\Klip", "User")
```

Close PowerShell and open a new one.

> **Newer version?** Replace `v0.10.0-alpha` with the newest tag from the [Releases page](https://github.com/Tribal-Chief-001/Klip/releases).

> **"Windows protected your PC" (SmartScreen)?** Klip is not signed with a paid certificate, so Windows may warn the first time. Click **More info → Run anyway**. Downloading with `Invoke-WebRequest` as above usually avoids the warning; a browser download usually triggers it. Only do this for the file from the real Releases page above.

## Step 2: start it

```powershell
klip start
klip status
```

`klip status` should show **STATE Active**. If Windows Defender Firewall asks whether to let `klip.exe` communicate on networks, tick **Private networks** and click **Allow access** (Klip connects directly to your other devices). There is no console window left open: the background part runs hidden.

`klip` alone opens a full-screen app (devices, pairing, history, settings). Press `q` to leave; Klip keeps running.

## Step 3: link your phone or another computer

Go to [Link your devices](05-link-devices.md). Do it once.

## Step 4: make it start by itself

Two things can start at login, and they do different jobs:

| What | What it does | Command |
|---|---|---|
| The **background part** (the one that syncs) | keeps Klip working | `klip install-service`, see below |
| The **tray icon** in the notification area | a menu to send an image, pause, open the Klip folder, send a file | `klip tray install` |

```powershell
klip install-service
```

It prints where it wrote a script (`register-service.ps1` in Klip's settings folder) and the line to run it, like

```powershell
powershell -ExecutionPolicy Bypass -File "C:\Users\you\AppData\Roaming\klip\klip\config\register-service.ps1"
```

Run that line. It registers a Windows task called **KlipDaemon** that starts Klip at every login and restarts it if it stops. If PowerShell says *Access is denied*, close it, open PowerShell with **Run as administrator**, and run the same line. This was never run on a real PC; if it fails, the simple fallback is to type `klip start` after you log in.

For the tray icon:

```powershell
klip tray install
klip tray
```

The tray starts through a tiny hidden script (`%LOCALAPPDATA%\Klip\start-tray.vbs`), so no black window flashes at login. If you installed the tray with an older Klip, run `klip tray install` again. Do not move `klip.exe` afterwards (if you do, run both install commands again). Details: [the tray icon](../TRAY_AND_TERMINAL_APP.md).

## Step 5: try it

Copy text here, paste on the other device. Send a file:

```powershell
klip send-file $env:USERPROFILE\Pictures\photo.jpg
```

More in [Everyday use](06-daily-use.md).

## Where things are

| What | Where |
|---|---|
| Settings, log, device key | `%APPDATA%\klip\klip\config\` (`config.json`, `daemon.log`, `keystore.json`). Paste that into File Explorer's address bar. |
| Files you receive | `%USERPROFILE%\Downloads\Klip` |
| Tray start-at-login | the `KlipTray` value in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` and `%LOCALAPPDATA%\Klip\start-tray.vbs` |
| Control channel | a named pipe `\\.\pipe\klip-<hash>`; only your Windows user can open it |

## Update

```powershell
klip stop
Invoke-WebRequest https://github.com/Tribal-Chief-001/Klip/releases/download/<NEWTAG>/klip-windows-x86_64.exe -OutFile $env:USERPROFILE\Klip\klip.exe
klip start
```

(Replace `<NEWTAG>` with the newest tag. If `klip.exe` is in use, run `klip stop` first and wait a few seconds.) Your links are kept.

## Remove Klip completely

```powershell
klip tray uninstall
Unregister-ScheduledTask -TaskName KlipDaemon -Confirm:$false
klip stop
Remove-Item $env:USERPROFILE\Klip -Recurse
```

(The `Unregister-ScheduledTask` line only matters if you ran the task script; an error saying it does not exist is fine.) To also forget this device's identity and links, delete `%APPDATA%\klip`. Received files in `Downloads\Klip` stay. Also remove this PC on your phone: open Klip, tap the PC's name, **Remove this device**. If you added the `Klip` folder to your PATH, remove it in *Settings → System → About → Advanced system settings → Environment Variables*.

Problems? [Troubleshooting](10-troubleshooting.md) · Questions? [FAQ](09-faq.md)
