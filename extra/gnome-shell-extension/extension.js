// Klip Bridge GNOME Shell Extension
// Bridges Meta.Selection directly to ~/.cache/klip/bridge.sock Unix Domain Socket
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';

export default class KlipBridgeExtension extends Extension {
    enable() {
        this._socketPath = GLib.build_filenamev([GLib.get_user_cache_dir(), 'klip', 'bridge.sock']);
        GLib.mkdir_with_parents(GLib.path_get_dirname(this._socketPath), 0o700);

        try {
            GLib.unlink(this._socketPath);
            let address = Gio.UnixSocketAddress.new(this._socketPath);
            this._service = new Gio.SocketService();
            this._service.add_address(address, Gio.SocketType.STREAM, Gio.SocketProtocol.DEFAULT, null);
            this._service.connect('incoming', (srv, conn) => this._onClientConnected(conn));
            this._service.start();
        } catch (e) {
            console.warn(`[KlipBridge] Failed to bind bridge socket: ${e}`);
        }
    }

    _onClientConnected(connection) {
        let display = global.display;
        let selection = display.get_selection();
        let sigId = selection.connect('owner-changed', (sel, type) => {
            if (type === Meta.SelectionType.SELECTION_CLIPBOARD) {
                selection.load_async(Meta.SelectionType.SELECTION_CLIPBOARD, 'text/plain;charset=utf-8', null, (s, res) => {
                    try {
                        let bytes = selection.load_finish(res);
                        let output = connection.get_output_stream();
                        output.write_bytes(bytes, null);
                    } catch (_) {}
                });
            }
        });
        connection.connect('closed', () => selection.disconnect(sigId));
    }

    disable() {
        if (this._service) {
            this._service.stop();
            this._service = null;
        }
        if (this._socketPath) {
            GLib.unlink(this._socketPath);
        }
    }
}
