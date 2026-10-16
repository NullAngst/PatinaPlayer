package dev.patina.player;

import android.Manifest;
import android.app.NativeActivity;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Bundle;

import java.util.ArrayList;
import java.util.List;

/**
 * The app's one screen. NativeActivity runs the Rust library's
 * android_main, which draws everything with Slint; this subclass adds what
 * the Rust side can't do on its own: asking for permissions, and wiring up
 * the Java helpers before the player starts.
 */
public class MainActivity extends NativeActivity {
    private static final int ASK = 1;

    static {
        // NativeActivity loads it too; loading it first lets Native.init
        // run before android_main starts.
        System.loadLibrary("patina_android");
    }

    @Override
    protected void onCreate(Bundle state) {
        Native.init(this);
        super.onCreate(state);
        askPermissions();
    }

    /** Music on the phone, and the media notification. */
    private void askPermissions() {
        List<String> want = new ArrayList<>();
        if (Build.VERSION.SDK_INT >= 33) {
            want.add(Manifest.permission.READ_MEDIA_AUDIO);
            want.add(Manifest.permission.POST_NOTIFICATIONS);
        } else {
            want.add(Manifest.permission.READ_EXTERNAL_STORAGE);
        }
        List<String> missing = new ArrayList<>();
        for (String p : want) {
            if (checkSelfPermission(p) != PackageManager.PERMISSION_GRANTED) {
                missing.add(p);
            }
        }
        if (missing.isEmpty()) {
            Native.permissionsDone(true);
        } else {
            requestPermissions(missing.toArray(new String[0]), ASK);
        }
    }

    @Override
    public void onRequestPermissionsResult(int code, String[] perms, int[] results) {
        super.onRequestPermissionsResult(code, perms, results);
        if (code != ASK) {
            return;
        }
        // The library can only see music with the audio permission. Without
        // it the app still plays servers, radio and podcasts.
        String audio = Build.VERSION.SDK_INT >= 33
            ? Manifest.permission.READ_MEDIA_AUDIO
            : Manifest.permission.READ_EXTERNAL_STORAGE;
        Native.permissionsDone(checkSelfPermission(audio) == PackageManager.PERMISSION_GRANTED);
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
        if (isFinishing()) {
            // The app is closing for good: the player stops with it, so
            // the notification goes too.
            PlaybackService.stop(getApplicationContext());
        }
    }
}
