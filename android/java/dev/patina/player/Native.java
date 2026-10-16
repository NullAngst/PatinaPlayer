package dev.patina.player;

import android.app.Activity;
import android.content.Context;
import android.media.AudioDeviceCallback;
import android.media.AudioDeviceInfo;
import android.media.AudioManager;
import android.net.wifi.WifiManager;
import android.os.Handler;
import android.os.Looper;

import java.lang.ref.WeakReference;
import java.util.ArrayList;
import java.util.List;

/**
 * The line between Java and the Rust library. Calls into Rust are the
 * native methods; Rust calls the static methods below, from its own
 * threads, and they hop to the main thread where Android needs it.
 */
final class Native {
    /** What the media session asks the player to do. Rust has the same list. */
    static final int PLAY = 1;
    static final int PAUSE = 2;
    static final int TOGGLE = 3;
    static final int NEXT = 4;
    static final int BACK = 5;
    static final int STOP = 6;
    static final int SEEK = 7;

    /** Playback state, as Rust reports it. */
    static final int STOPPED = 0;
    static final int PLAYING = 1;
    static final int PAUSED = 2;

    static Context app;
    static Handler main;
    private static WeakReference<Activity> activity = new WeakReference<>(null);
    private static WifiManager.MulticastLock multicast;

    private Native() {}

    /** Called by MainActivity before android_main starts. */
    static void init(Activity a) {
        app = a.getApplicationContext();
        activity = new WeakReference<>(a);
        main = new Handler(Looper.getMainLooper());
        nativeInit();
        // DLNA servers are found by a multicast search, which Android drops
        // unless an app holds this.
        if (multicast == null) {
            WifiManager wifi = (WifiManager) app.getSystemService(Context.WIFI_SERVICE);
            if (wifi != null) {
                multicast = wifi.createMulticastLock("patinaplayer-dlna");
                multicast.setReferenceCounted(false);
                multicast.acquire();
            }
        }
        watchDevices();
    }

    // ----- Into Rust -----

    /** Keeps this class, so Rust's threads can find it. */
    static native void nativeInit();

    /** The permission question is answered; true if music can be read. */
    static native void permissionsDone(boolean audio);

    /** One of the constants above; arg is the position in ms for SEEK. */
    static native void command(int cmd, long arg);

    /** The output devices there are now. */
    static native void devicesChanged(int[] ids, String[] names, boolean[] usb);

    // ----- From Rust -----

    /** What's playing, for the media session and the notification. */
    static void onState(String title, String artist, String album, String artPath,
                        long durationMs, long positionMs, int state) {
        main.post(() -> PlaybackService.update(app, title, artist, album, artPath,
            durationMs, positionMs, state));
    }

    /**
     * Put a USB DAC's mixer into bit-perfect mode at this format. Returns
     * null once it's set, or why not. rate 0 clears it again.
     */
    static String setBitPerfect(int deviceId, int rate, int bits, int channels) {
        return BitPerfect.set(app, deviceId, rate, bits, channels);
    }

    /** Back with nowhere left to go: out of the way, still playing. */
    static void moveToBack() {
        main.post(() -> {
            Activity a = activity.get();
            if (a != null) {
                a.moveTaskToBack(true);
            }
        });
    }

    /** The app is closing: take the notification down. */
    static void stopped() {
        main.post(() -> PlaybackService.stop(app));
    }

    // ----- Devices -----

    private static void watchDevices() {
        AudioManager am = (AudioManager) app.getSystemService(Context.AUDIO_SERVICE);
        am.registerAudioDeviceCallback(new AudioDeviceCallback() {
            @Override
            public void onAudioDevicesAdded(AudioDeviceInfo[] added) {
                reportDevices(am);
            }

            @Override
            public void onAudioDevicesRemoved(AudioDeviceInfo[] removed) {
                reportDevices(am);
            }
        }, main);
        reportDevices(am);
    }

    private static void reportDevices(AudioManager am) {
        List<AudioDeviceInfo> keep = new ArrayList<>();
        for (AudioDeviceInfo d : am.getDevices(AudioManager.GET_DEVICES_OUTPUTS)) {
            if (label(d.getType()) != null) {
                keep.add(d);
            }
        }
        int[] ids = new int[keep.size()];
        String[] names = new String[keep.size()];
        boolean[] usb = new boolean[keep.size()];
        for (int i = 0; i < keep.size(); i++) {
            AudioDeviceInfo d = keep.get(i);
            ids[i] = d.getId();
            String product = d.getProductName() == null ? "" : d.getProductName().toString();
            String kind = label(d.getType());
            names[i] = product.isEmpty() || product.equals(android.os.Build.MODEL)
                ? kind
                : kind + " (" + product + ")";
            usb[i] = BitPerfect.isUsb(d);
        }
        devicesChanged(ids, names, usb);
    }

    /** Devices worth offering, by kind. Null for the rest (earpiece, buses). */
    private static String label(int type) {
        switch (type) {
            case AudioDeviceInfo.TYPE_BUILTIN_SPEAKER: return "Speaker";
            case AudioDeviceInfo.TYPE_WIRED_HEADPHONES: return "Headphones";
            case AudioDeviceInfo.TYPE_WIRED_HEADSET: return "Headset";
            case AudioDeviceInfo.TYPE_USB_DEVICE: return "USB audio";
            case AudioDeviceInfo.TYPE_USB_HEADSET: return "USB headset";
            case AudioDeviceInfo.TYPE_BLUETOOTH_A2DP: return "Bluetooth";
            case AudioDeviceInfo.TYPE_HDMI: return "HDMI";
            case AudioDeviceInfo.TYPE_LINE_ANALOG: return "Line out";
            case AudioDeviceInfo.TYPE_LINE_DIGITAL: return "Digital out";
            default: return null;
        }
    }
}
