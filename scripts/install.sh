#!/usr/bin/env bash
# ==============================================================================
# Klip Linux Production Installer & Service Configurator
# Zero-friction setup: installs binary, systemd user service, tray autostart & PATH
# ==============================================================================
set -euo pipefail

BOLD="$(tput bold 2>/dev/null || echo '')"
GREEN="$(tput setaf 2 2>/dev/null || echo '')"
YELLOW="$(tput setaf 3 2>/dev/null || echo '')"
RED="$(tput setaf 1 2>/dev/null || echo '')"
CYAN="$(tput setaf 6 2>/dev/null || echo '')"
RESET="$(tput sgr0 2>/dev/null || echo '')"

info()    { echo "${CYAN}${BOLD}[INFO]${RESET} $*"; }
success() { echo "${GREEN}${BOLD}[✓]${RESET} $*"; }
warn()    { echo "${YELLOW}${BOLD}[!]${RESET} $*"; }
error()   { echo "${RED}${BOLD}[✗]${RESET} $*" >&2; }

echo "${CYAN}${BOLD}"
cat << 'EOF'
 _    _ _       
| | _| (_)_ __  
| |/ / | | '_ \ 
|   <| | | |_) |
|_|\_\_|_| .__/ 
         |_|    
End-to-End Encrypted Clipboard Sync
EOF
echo "${RESET}"

# 1. Determine target directory
INSTALL_DIR="${HOME}/.local/bin"
mkdir -p "${INSTALL_DIR}"

KLIP_BIN="${INSTALL_DIR}/klip"

# 2. Locate or compile klip binary
find_or_build_binary() {
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

    if [[ -f "${REPO_ROOT}/target/release/klip" ]]; then
        info "Found compiled binary at target/release/klip"
        cp -f "${REPO_ROOT}/target/release/klip" "${KLIP_BIN}"
    elif [[ -f "${REPO_ROOT}/target/debug/klip" ]]; then
        info "Found debug binary at target/debug/klip"
        cp -f "${REPO_ROOT}/target/debug/klip" "${KLIP_BIN}"
    elif command -v cargo >/dev/null 2>&1 && [[ -f "${REPO_ROOT}/Cargo.toml" ]]; then
        info "Building klip from source in ${REPO_ROOT}..."
        (cd "${REPO_ROOT}" && cargo build --release --bin klip)
        cp -f "${REPO_ROOT}/target/release/klip" "${KLIP_BIN}"
    else
        info "Attempting to download latest prebuilt release from GitHub..."
        ARCH="$(uname -m)"
        case "${ARCH}" in
            x86_64)  TARGET="linux-x86_64" ;;
            *)       error "Unsupported architecture: ${ARCH}. Prebuilt releases are currently available for x86_64."; exit 1 ;;
        esac

        DOWNLOAD_URL="https://github.com/Tribal-Chief-001/Klip/releases/download/v0.12.0-alpha/klip-${TARGET}"
        RELEASES_API="https://api.github.com/repos/Tribal-Chief-001/Klip/releases"

        # Attempt dynamic discovery from GitHub Releases API (mirroring install.ps1)
        if command -v curl >/dev/null 2>&1; then
            API_RESP="$(curl -sSL -H "User-Agent: Klip-Installer" "${RELEASES_API}" 2>/dev/null || true)"
            if [[ -n "${API_RESP}" ]]; then
                DISCOVERED_URL="$(echo "${API_RESP}" | grep -o "https://[^\"]*${TARGET}[^\"]*" | head -n 1 || true)"
                if [[ -n "${DISCOVERED_URL}" ]]; then
                    DOWNLOAD_URL="${DISCOVERED_URL}"
                fi
            fi
        fi

        if curl -fsSL "${DOWNLOAD_URL}" -o "${KLIP_BIN}" 2>/dev/null; then
            chmod +x "${KLIP_BIN}"
            success "Downloaded prebuilt klip binary to ${KLIP_BIN}"
        else
            # Try latest release download redirect if API/direct tag was unavailable
            LATEST_URL="https://github.com/Tribal-Chief-001/Klip/releases/latest/download/klip-${TARGET}"
            if curl -fsSL "${LATEST_URL}" -o "${KLIP_BIN}" 2>/dev/null; then
                chmod +x "${KLIP_BIN}"
                success "Downloaded latest klip binary to ${KLIP_BIN}"
            else
                warn "Could not download prebuilt binary from ${DOWNLOAD_URL}. Falling back to cargo install..."
                if command -v cargo >/dev/null 2>&1; then
                    cargo install --git https://github.com/Tribal-Chief-001/Klip klip-cli --bin klip --root "${HOME}/.local"
                else
                    error "Neither prebuilt binary nor cargo found. Please install Rust or download klip manually."
                    exit 1
                fi
            fi
        fi
    fi

    chmod +x "${KLIP_BIN}"
    success "Installed klip binary to ${KLIP_BIN}"
}

find_or_build_binary

# 3. Verify PATH
verify_path() {
    if [[ ":$PATH:" != *":${INSTALL_DIR}:"* ]]; then
        warn "${INSTALL_DIR} is not currently in your PATH!"
        SHELL_RC=""
        if [[ -n "${BASH_VERSION:-}" ]] || [[ "${SHELL:-}" == *"bash"* ]]; then
            SHELL_RC="${HOME}/.bashrc"
        elif [[ -n "${ZSH_VERSION:-}" ]] || [[ "${SHELL:-}" == *"zsh"* ]]; then
            SHELL_RC="${HOME}/.zshrc"
        else
            SHELL_RC="${HOME}/.profile"
        fi

        if [[ -n "${SHELL_RC}" && -f "${SHELL_RC}" ]]; then
            echo 'export PATH="$HOME/.local/bin:$PATH"' >> "${SHELL_RC}"
            info "Added export PATH=\"\$HOME/.local/bin:\$PATH\" to ${SHELL_RC}"
            export PATH="${INSTALL_DIR}:${PATH}"
        fi
    else
        success "PATH includes ${INSTALL_DIR}"
    fi
}

verify_path

# 4. Configure systemd user service
configure_systemd() {
    if command -v systemctl >/dev/null 2>&1 && systemctl --user status >/dev/null 2>&1; then
        info "Configuring systemd user service..."
        SYSTEMD_USER_DIR="${HOME}/.config/systemd/user"
        mkdir -p "${SYSTEMD_USER_DIR}"

        SERVICE_FILE="${SYSTEMD_USER_DIR}/klip.service"
        cat > "${SERVICE_FILE}" << EOF
[Unit]
Description=Klip - End-to-end encrypted clipboard sync
Documentation=https://github.com/Tribal-Chief-001/Klip
After=network.target

[Service]
Type=simple
ExecStart=${KLIP_BIN} start --foreground
Restart=on-failure
RestartSec=3s
Slice=app.slice

[Install]
WantedBy=default.target
EOF
        systemctl --user daemon-reload
        systemctl --user enable --now klip.service
        success "systemd user service 'klip.service' enabled and started"
    else
        warn "systemd user session not detected. Starting klip daemon directly in background..."
        "${KLIP_BIN}" start || true
    fi
}

configure_systemd

# 5. Configure desktop tray autostart
configure_tray() {
    if [[ -n "${DISPLAY:-}" ]] || [[ -n "${WAYLAND_DISPLAY:-}" ]]; then
        info "Desktop session detected. Configuring tray autostart..."
        AUTOSTART_DIR="${HOME}/.config/autostart"
        mkdir -p "${AUTOSTART_DIR}"
        DESKTOP_FILE="${AUTOSTART_DIR}/klip-tray.desktop"

        cat > "${DESKTOP_FILE}" << EOF
[Desktop Entry]
Type=Application
Name=Klip Tray
Comment=Klip clipboard synchronization tray
Exec=${KLIP_BIN} tray run
Icon=klip
Terminal=false
Categories=Utility;
X-GNOME-Autostart-enabled=true
EOF
        chmod +x "${DESKTOP_FILE}"
        success "Tray autostart entry created at ${DESKTOP_FILE}"
    fi
}

configure_tray

# 6. Verify installation
echo ""
info "Verifying Klip daemon health..."
sleep 1
if "${KLIP_BIN}" status; then
    echo ""
    success "Klip installation completed successfully!"
    echo "  - CLI Piping:     echo 'hello' | klip copy   &&   klip paste"
    echo "  - Interactive UI: klip"
    echo "  - Pairing:        klip pair"
    echo "  - Service Logs:   journalctl --user -u klip.service -f"
else
    warn "Klip installed, but daemon might still be initializing. Try: klip status"
fi
