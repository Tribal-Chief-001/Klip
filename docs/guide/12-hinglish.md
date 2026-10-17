# Klip — Hinglish quick guide (aasaan bhasha mein)

Klip kya hai? Ek device pe copy karo, doosre pe paste karo. Files aur folders bhi bhej sakte ho. Sab kuch encrypted hai, koi account nahi, koi cloud nahi.

> Yeh **alpha** software hai. Kaam karta hai, par abhi naya hai. Linux + Android sabse zyada test hua hai; Mac aur Windows sirf automatic test machine pe test hue hain, kisi asli Mac/PC pe nahi.

## 3 step, har device pe

1. **Install** karo (computer pe program, phone pe app).
2. **Link** karo (sirf ek baar): ek device QR dikhata hai, doosra scan karta hai, pehle device pe `y` dabake approve karte ho.
3. **Use** karo: copy karo, doosre pe paste karo.

**Zaroori:** dono devices **ek saath on aur online** hone chahiye. Agar doosra device band hai to copy baad mein nahi pahunchegi (koi mailbox nahi hai).

## Phone (Android)

1. Phone ke browser mein <https://github.com/Tribal-Chief-001/Klip/releases> kholo, latest release mein **`Klip-Android.apk`** download karo.
2. Kholo → "Install unknown apps" allow karo → **Install**. Play Protect bole to **More details → Install anyway**.
3. Klip kholo → upar **Setup** dabao. Yeh bahut zaroori hai: Notifications allow karo, **Unrestricted battery use** do, aur apne phone brand (Vivo/Xiaomi/Oppo/Samsung…) ka auto-start on karo. Warna screen band hone pe Android Klip ko band kar dega.

## Laptop / PC

**Linux**
```bash
mkdir -p ~/.local/bin && cd ~/Downloads
curl -L -O https://github.com/Tribal-Chief-001/Klip/releases/download/v0.10.0-alpha/klip-linux-x86_64
install -Dm755 klip-linux-x86_64 ~/.local/bin/klip
klip start && klip status
```
Auto-start: `klip install-service` phir `systemctl --user enable --now klip.service`. Poori detail: [Linux guide](01-linux.md).

**Mac (sirf Apple chip M1/M2/…)**
```bash
mkdir -p ~/.local/bin && cd ~/Downloads
curl -L -O https://github.com/Tribal-Chief-001/Klip/releases/download/v0.10.0-alpha/klip-macos-arm64
install -m755 klip-macos-arm64 ~/.local/bin/klip
xattr -d com.apple.quarantine ~/.local/bin/klip
klip start && klip status
```
"Developer cannot be verified" aaye to System Settings → Privacy & Security → **Open Anyway**. Poori detail: [macOS guide](02-macos.md).

**Windows (PowerShell)**
```powershell
mkdir $env:USERPROFILE\Klip -Force; cd $env:USERPROFILE\Klip
Invoke-WebRequest https://github.com/Tribal-Chief-001/Klip/releases/download/v0.10.0-alpha/klip-windows-x86_64.exe -OutFile klip.exe
.\klip.exe start; .\klip.exe status
```
SmartScreen bole to **More info → Run anyway**. Firewall poochhe to **Private networks → Allow**. Poori detail (PATH, auto-start): [Windows guide](03-windows.md).

(Naya version aaya ho to Releases page se latest tag lo; `v0.10.0-alpha` ki jagah wahi likho.)

## Link karna (ek baar)

1. Computer pe: `klip pair` → QR code aata hai. **Window khuli rakho** (2 minute chalta hai, ek baar).
2. Phone pe: Klip → **Add a device** → **Scan a code** → QR scan karo. (Camera na chale to **Paste a ticket**.)
3. Computer pe sawal aayega "wants to pair … Approve?" → fingerprint match karo → `y` dabao.
4. Ho gaya. Test: ek word copy karo, doosre pe paste karo.

Detail: [Link guide](05-link-devices.md).

## Roz ka use

| Kya karna hai | Kaise |
|---|---|
| Computer se phone pe text | Bas copy karo, phone pe paste karo |
| Phone se computer pe text | **Share → Klip**, ya Klip tile, ya notification ka **Send clipboard** button (Android khud copy nahi dekhne deta) |
| Computer ki image bhejna | Copy karo, phir tray menu → "Send the last copied image", ya `klip share` |
| Computer se file | `klip send-file report.pdf` |
| Computer se folder | `klip send-file ~/Pictures/Trip` |
| Phone se file | Share → **Send as a file with Klip** |
| Phone se folder | Klip → **Send a folder** |
| Mili hui files kahan | `Downloads/Klip` |
| Kuch der ke liye roko | `klip pause 600`, wapas: `klip resume` |
| Device hatao | `klip unpair "Pixel 8"` |

Detail: [Everyday use](06-daily-use.md).

## Kuch gadbad ho to

1. `klip status` chalao — device hara (connected) hai?
2. Phone ka **Setup** poora karo (battery Unrestricted).
3. Dono devices ke paas internet hai? Ya same Wi-Fi pe `"relay": "lan"` try karo.
4. [Troubleshooting](10-troubleshooting.md) mein apna symptom dhundho, ya [FAQ](09-faq.md) padho.
5. Phir bhi nahi chala to [GitHub issue](https://github.com/Tribal-Chief-001/Klip/issues) kholo — apna system, kaunsa step, exact message, aur `daemon.log` ki last lines likho. **Ticket (`klip1…`) ya `keystore.json` kabhi mat paste karo.**

← [Guide start](README.md)
