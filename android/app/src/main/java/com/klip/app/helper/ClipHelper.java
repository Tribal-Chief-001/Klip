package com.klip.app.helper;

import android.content.ClipData;
import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;

import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.File;
import java.io.InputStream;
import java.net.InetAddress;
import java.net.Socket;
import java.security.MessageDigest;
import java.security.SecureRandom;
import javax.crypto.Mac;
import javax.crypto.spec.SecretKeySpec;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Optional helper that gives Klip automatic "copy on this phone, paste on the other device".
 *
 * Why it exists: since Android 10 only the focused app (or the default keyboard) may read the clipboard;
 * the old trick of watching the system log was closed in Android 13 (background apps get no log access).
 * The Android shell user and root, however, may read it any time. So this class is started as one of
 * them with `app_process` (once from a computer over USB debugging, or by Klip itself on a rooted phone),
 * listens for clipboard changes through the system service, and passes each copied text to the Klip app
 * over a local socket. It is plain framework code (reflection on hidden classes, the same approach as
 * scrcpy) and does not depend on any other Klip class.
 *
 * Transport: a loopback TCP connection to the app (SELinux does not let the shell user connect to an app's
 * Unix socket). Any app could listen on that port, so both sides prove they know a secret token (HMAC
 * challenge-response) before any clip is sent; the token is never sent. The helper exits when the apk
 * disappears or the app sends 'Q'.
 *
 * Arguments: port token apkPath.
 */
public final class ClipHelper {
    private static final String LISTENER = "android.content.IOnPrimaryClipChangedListener";
    private static final int MAX_BYTES = 8 * 1024 * 1024;
    private static final boolean DEBUG = System.getenv("KLIP_HELPER_DEBUG") != null;

    private static volatile Socket sock;
    private static volatile DataOutputStream out;
    private static final Object LOCK = new Object();
    private static final ExecutorService WORK = Executors.newSingleThreadExecutor();

    private static Object clipboard;
    private static Method getPrimaryClip;
    private static String callerPackage;

    public static void main(String[] args) throws Exception {
        final int port = Integer.parseInt(args[0]);
        final byte[] token = args[1].getBytes(StandardCharsets.UTF_8);
        final String apk = args.length > 2 ? args[2] : null;
        callerPackage = System.getenv("KLIP_HELPER_PKG") != null ? System.getenv("KLIP_HELPER_PKG") : "com.android.shell";
        dropRootToShell();

        Class<?> sm = Class.forName("android.os.ServiceManager");
        IBinder b = (IBinder) sm.getMethod("getService", String.class).invoke(null, "clipboard");
        Class<?> stub = Class.forName("android.content.IClipboard$Stub");
        clipboard = stub.getMethod("asInterface", IBinder.class).invoke(null, b);
        getPrimaryClip = find("getPrimaryClip");
        Method add = find("addPrimaryClipChangedListener");

        // A Binder that the clipboard service calls (transaction 1 = dispatchPrimaryClipChanged), and an
        // object implementing the hidden interface so the generated proxy accepts it.
        final Binder binder = new Binder() {
            @Override
            protected boolean onTransact(int code, Parcel data, Parcel reply, int flags) throws android.os.RemoteException {
                if (code == 1) {
                    if (DEBUG) System.out.println("clipboard changed");
                    WORK.execute(ClipHelper::onChanged);
                    return true;
                }
                return super.onTransact(code, data, reply, flags);
            }
        };
        Object listener = Proxy.newProxyInstance(
                ClipHelper.class.getClassLoader(),
                new Class<?>[]{Class.forName(LISTENER)},
                (proxy, method, a) -> "asBinder".equals(method.getName()) ? binder : null);
        add.invoke(clipboard, build(add, listener));

        // Connection to the app, retried forever. Also the exit conditions.
        new Thread(() -> connectLoop(port, token, apk), "klip-helper-link").start();
        System.out.println("klip helper running as uid " + android.os.Process.myUid());
        Thread.currentThread().join();
    }

    /**
     * Started as root (a rooted phone), the clipboard service delivers changes but reading them as uid 0
     * failed in testing, while the shell user (2000, package com.android.shell) works. So become shell.
     */
    private static void dropRootToShell() {
        try {
            if (android.os.Process.myUid() != 0) return;
            Class<?> p = Class.forName("android.os.Process");
            p.getMethod("setGid", int.class).invoke(null, 2000); // group first: it cannot be changed after the uid
            p.getMethod("setUid", int.class).invoke(null, 2000);
        } catch (Throwable t) {
            if (DEBUG) t.printStackTrace();
        }
    }

    private static void onChanged() {
        byte[] bytes;
        try {
            Object clip = getPrimaryClip.invoke(clipboard, build(getPrimaryClip, null));
            if (DEBUG) System.out.println("read clip: " + (clip == null ? "null" : clip.getClass().getName()));
            if (!(clip instanceof ClipData)) return;
            ClipData data = (ClipData) clip;
            if (data.getItemCount() == 0) return;
            // Password managers mark what they copy as sensitive: do not send those automatically.
            // (The person can still send one on purpose with Klip's Send button.)
            android.os.PersistableBundle extras = data.getDescription().getExtras();
            if (extras != null && extras.getBoolean("android.content.extra.IS_SENSITIVE", false)) return;
            CharSequence text = data.getItemAt(0).getText();
            if (text == null || text.length() == 0) return;
            bytes = text.toString().getBytes(StandardCharsets.UTF_8);
            if (bytes.length > MAX_BYTES) return;
        } catch (Throwable t) {
            // Reading can fail (a clip over about 1 MB does not fit through Binder): skip this clip, keep the link.
            if (DEBUG) t.printStackTrace();
            return;
        }
        try {
            synchronized (LOCK) {
                if (out == null) return; // the app is not listening right now; nothing to deliver
                out.writeInt(bytes.length);
                out.write(bytes);
                out.flush();
            }
        } catch (Throwable t) {
            if (DEBUG) t.printStackTrace();
            closeSocket(); // the link loop reconnects
        }
    }

    private static void connectLoop(int port, byte[] token, String apk) {
        while (true) {
            if (apk != null && !new File(apk).exists()) System.exit(0); // Klip was uninstalled
            try {
                Socket s = new Socket(InetAddress.getByName("127.0.0.1"), port);
                s.setTcpNoDelay(true);
                DataInputStream in = new DataInputStream(s.getInputStream());
                DataOutputStream o = new DataOutputStream(s.getOutputStream());
                if (!handshake(in, o, token)) { // whatever listens on that port does not know the token
                    s.close();
                    Thread.sleep(10_000);
                    continue;
                }
                synchronized (LOCK) {
                    sock = s;
                    out = o;
                }
                int c;
                while ((c = in.read()) != -1) {
                    if (c == 'Q') System.exit(0); // the app asked the helper to stop
                }
            } catch (Throwable ignored) {
                // not up yet, or the app restarted: try again
            }
            closeSocket();
            try {
                Thread.sleep(3_000);
            } catch (InterruptedException e) {
                return;
            }
        }
    }

    /** Mutual proof of the shared token, without sending it. Returns false if the other side fails. */
    private static boolean handshake(DataInputStream in, DataOutputStream out, byte[] token) throws Exception {
        byte[] nh = new byte[16];
        new SecureRandom().nextBytes(nh);
        out.write(nh);
        out.flush();
        byte[] ns = new byte[16];
        byte[] proof = new byte[32];
        in.readFully(ns);
        in.readFully(proof);
        if (!MessageDigest.isEqual(proof, mac(token, "server", nh, ns))) return false;
        out.write(mac(token, "helper", ns, nh));
        out.flush();
        return true;
    }

    private static byte[] mac(byte[] token, String role, byte[] a, byte[] b) throws Exception {
        Mac m = Mac.getInstance("HmacSHA256");
        m.init(new SecretKeySpec(token, "HmacSHA256"));
        m.update(role.getBytes(StandardCharsets.UTF_8));
        m.update(a);
        m.update(b);
        return m.doFinal();
    }

    private static void closeSocket() {
        synchronized (LOCK) {
            out = null;
            try {
                if (sock != null) sock.close();
            } catch (Throwable ignored) {
            }
            sock = null;
        }
    }

    private static Method find(String name) {
        for (Method m : clipboard.getClass().getMethods()) {
            if (m.getName().equals(name)) return m;
        }
        throw new IllegalStateException("clipboard service has no " + name);
    }

    /** Fill the arguments of a clipboard-service method whose signature differs between Android versions. */
    private static Object[] build(Method m, Object listener) {
        Class<?>[] t = m.getParameterTypes();
        Object[] a = new Object[t.length];
        int strings = 0;
        for (int i = 0; i < t.length; i++) {
            if (t[i].getName().equals(LISTENER)) a[i] = listener;
            else if (t[i] == String.class) a[i] = strings++ == 0 ? callerPackage : null; // package, then attribution tag
            else if (t[i] == int.class) a[i] = 0; // user 0, default device
        }
        return a;
    }
}
